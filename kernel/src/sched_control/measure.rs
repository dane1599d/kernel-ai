//! Reading the Cores' accounting from a kernel thread: snapshots of every CPU and
//! the exact wake-to-run samples, drained into the heap often enough that no
//! Core's 256-sample ring overwrites one first.
//!
//! Sample sequence numbers belong to one Core's ring, so each Core is drained on
//! its own; a thread that migrates leaves its earlier samples behind on the Core
//! it left, and they are still drained there. Every copy happens under one core
//! lock at a time, into a fixed buffer, and is pushed onto the heap after.

use crate::{hpet, process, smp, threads};
use alloc::vec;
use alloc::vec::Vec;
use kabi::Error;
use kabi::sched::{SchedStats, ThreadRecord, class};
use sched::stats::{SAMPLES, Snapshot, Window, percentile};
use sched::{Class, MAX_THREADS, Tid};

/// How often samples are drained while a window runs.
pub const DRAIN_NS: u64 = 100_000_000;

/// Every CPU's counters at one moment.
pub struct Snapshots {
    pub cores: Vec<Snapshot>,
    /// Each CPU's idle thread.
    pub idle: Vec<Tid>,
    pub ejections: u64,
}

impl Snapshots {
    pub fn new() -> Self {
        let cpus = smp::online();
        Self {
            cores: vec![Snapshot::EMPTY; cpus],
            idle: vec![0; cpus],
            ejections: 0,
        }
    }

    /// Takes every CPU's counters now.
    pub fn take(&mut self) {
        self.ejections = 0;
        for (cpu, (snapshot, idle)) in self.cores.iter_mut().zip(&mut self.idle).enumerate() {
            threads::with_core(cpu, |core, now| {
                core.snapshot_into(now, snapshot);
                *idle = core.idle();
                self.ejections += u64::from(core.ejections());
            });
        }
    }

    /// Per thread, summed over CPUs: the window from `earlier` to these counters,
    /// as (CPU, wakes, preemptions, longest wait); and the idle CPU time.
    pub fn since(&self, earlier: &Snapshots) -> (Vec<[u64; 4]>, u64) {
        let mut sums = vec![[0u64; 4]; MAX_THREADS];
        let mut idle_ns = 0;
        let mut window = alloc::boxed::Box::new(Window::EMPTY);
        for (cpu, (now, then)) in self.cores.iter().zip(&earlier.cores).enumerate() {
            now.window_into(then, &mut window);
            idle_ns += window.threads[self.idle[cpu]].cpu_ns;
            for (tid, sum) in sums.iter_mut().enumerate() {
                if self.idle.contains(&tid) {
                    continue;
                }
                let t = &window.threads[tid];
                sum[0] += t.cpu_ns;
                sum[1] += t.wakes;
                sum[2] += t.preemptions;
                sum[3] = sum[3].max(t.max_wait_ns);
            }
        }
        (sums, idle_ns)
    }
}

/// Drains wake-to-run samples, Core by Core.
pub struct Drain {
    next: Vec<[u64; MAX_THREADS]>,
}

impl Drain {
    /// Starts at the samples every Core holds now: only later ones are drained.
    pub fn start() -> Self {
        let mut next = vec![[0u64; MAX_THREADS]; smp::online()];
        for (cpu, next) in next.iter_mut().enumerate() {
            threads::with_core(cpu, |core, _| {
                for (tid, seq) in next.iter_mut().enumerate() {
                    *seq = core.stats().counters(tid).samples;
                }
            });
        }
        Self { next }
    }

    /// Moves the new samples of the threads `want` names into `out`, by thread.
    /// Samples a ring already overwrote are lost; the rest are kept in order.
    pub fn take(&mut self, want: impl Fn(Tid) -> bool, out: &mut [Vec<u32>]) {
        let mut buf = [0u32; SAMPLES];
        for (cpu, next) in self.next.iter_mut().enumerate() {
            for (tid, seq) in next.iter_mut().enumerate().filter(|(tid, _)| want(*tid)) {
                let copied = threads::with_core(cpu, |core, _| {
                    let end = core.stats().counters(tid).samples;
                    let from = (*seq).max(end.saturating_sub(SAMPLES as u64));
                    *seq = end;
                    core.stats().samples(tid, from, end, &mut buf).unwrap_or(0)
                });
                if let (Some(n), Some(samples)) = (copied, out.get_mut(tid)) {
                    samples.extend_from_slice(&buf[..n]);
                }
            }
        }
    }
}

/// Sleeps until `deadline`, draining every [`DRAIN_NS`], unless `stop()` says to
/// end early (checked after each drain); returns false then.
pub fn wait_draining(
    deadline: u64,
    drain: &mut Drain,
    want: impl Fn(Tid) -> bool + Copy,
    out: &mut [Vec<u32>],
    stop: impl Fn() -> bool,
) -> bool {
    loop {
        let now = hpet::nanos();
        if now >= deadline {
            drain.take(want, out);
            return true;
        }
        let until = deadline.min(now + DRAIN_NS);
        threads::sleep_until_unless(until, &stop);
        drain.take(want, out);
        if stop() {
            return false;
        }
    }
}

/// SCHED_STATS's window: every thread's numbers over `window_ns`, watched by the
/// calling process's thread; Dead if the process is killed meanwhile.
pub fn observe(slot: usize, window_ns: u64, out: &mut SchedStats) -> Result<(), Error> {
    let mut before = Snapshots::new();
    let mut after = Snapshots::new();
    before.take();
    let mut drain = Drain::start();
    let mut samples = vec![Vec::new(); MAX_THREADS];
    let start = hpet::nanos();
    let stop = || process::killed(slot);
    if !wait_draining(start + window_ns, &mut drain, |_| true, &mut samples, stop) {
        return Err(Error::Dead);
    }
    after.take();
    let (sums, idle_ns) = after.since(&before);
    let window = after.cores.first().map_or(0, |s| s.at) - before.cores.first().map_or(0, |s| s.at);
    out.window_ns = window;
    out.busy_ns = (window * out.cpus).saturating_sub(idle_ns);
    let pids = process::pids_by_tid();
    let classes =
        threads::with_core(0, |core, _| core.class_table()).unwrap_or([Class::Normal; MAX_THREADS]);
    let mut count = 0;
    for tid in 0..MAX_THREADS {
        let [cpu_ns, wakes, preemptions, _] = sums[tid];
        if cpu_ns == 0 && wakes == 0 || after.idle.contains(&tid) {
            continue;
        }
        let Some(record) = out.threads.get_mut(count) else {
            break;
        };
        let mut sorted: Vec<u64> = samples[tid].iter().map(|&ns| u64::from(ns)).collect();
        sorted.sort_unstable();
        *record = ThreadRecord {
            tid: tid as u64,
            pid: pids[tid],
            class: match classes.get(tid) {
                Some(Class::Background) => class::BACKGROUND,
                _ => class::NORMAL,
            },
            cpu_ns,
            wakes,
            preemptions,
            samples: sorted.len() as u64,
            p50_ns: percentile(&sorted, 50),
            p99_ns: percentile(&sorted, 99),
            max_ns: sorted.last().copied().unwrap_or(0),
        };
        count += 1;
    }
    out.count = count as u64;
    Ok(())
}
