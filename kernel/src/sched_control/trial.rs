//! Kernel-owned policy trials (SCHED_TRIAL). A kernel thread alternates windows
//! of the installed policy (baseline) and the candidate, which every CPU's Core
//! holds staged in its standby slot; `sched::judge` keeps the candidate or not.
//!
//! The trial belongs to the kernel. The process that asked can die and the trial
//! still ends cleanly; in fact it reverts at once, since nobody is left to answer
//! for it. Reverting never waits for the driver thread:
//! - the timer tick on each CPU runs [`guard`] in candidate windows: a runnable
//!   wait over 100 ms or a watchdog ejection reverts that CPU's Core through the
//!   allocation-free standby, at the first tick past the breach, and asks every
//!   other CPU to revert at its next tick;
//! - a person's policy switch, the exit of the starter or the target, or a kill
//!   (a runaway, a crash, the supervisor's) reverts every CPU from that thread.
//!
//! Measurements: each window drops its first 200 ms, then takes the Cores'
//! counters and drains the target's exact wake-to-run samples (floored at
//! 100 us); background is every other process, by the founder's choice of target,
//! never by the labels.

use super::guard::{ABORT, ARMED, LAST_WAIT, TRIP_PID, TRIP_REASON, claim, last_trip};
use super::ledger::{describe, reason_code};
use super::measure::{Drain, Snapshots, wait_draining};
use super::{Control, class_table, ids, with};
use crate::{hpet, log, metrics, process, smp, threads};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use kabi::sched::{WindowRecord, reason, state};
use kabi::{Error, ExitReason};
use sched::judge::{DISCARD_NS, LATENCY_FLOOR_NS, Pair, Reason, WindowMeasure};
use sched::spec::PolicySpec;
use sched::{MAX_THREADS, SchedPolicy, Tid};

/// What is tried.
#[derive(Clone, Copy)]
pub enum Candidate {
    Spec(PolicySpec),
    /// A policy outside the catalog, which the system call never accepts: the
    /// `tune-revert` case's deliberately starving one.
    Test(fn() -> Box<dyn SchedPolicy>),
}

impl Candidate {
    fn build(&self) -> Result<Box<dyn SchedPolicy>, Error> {
        match self {
            Candidate::Spec(spec) => spec.build().map_err(|_| Error::Invalid),
            Candidate::Test(make) => Ok(make()),
        }
    }
}

/// A trial to run.
#[derive(Clone)]
pub struct Request {
    pub candidate: Candidate,
    pub target_pid: u64,
    pub background: Vec<u64>,
    pub window_ns: u64,
    pub pairs: usize,
    /// The process that asked (0 for the kernel).
    pub starter_pid: u64,
}

/// The trial that runs now.
pub(crate) struct Active {
    id: u64,
    request: Request,
}

/// The driver thread, plus one (0 = none).
static DRIVER: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn running(control: &Control) -> bool {
    control.active.is_some()
}

fn mine(control: &Control, id: u64) -> bool {
    control
        .active
        .as_ref()
        .is_some_and(|active| active.id == id)
}

/// Reverts every CPU and empties every standby slot into `old`.
fn unstage_all(old: &mut Vec<Box<dyn SchedPolicy>>) {
    ARMED.store(false, Ordering::Release);
    for cpu in 0..smp::online() {
        threads::with_core(cpu, |core, now| {
            core.revert(now);
            old.extend(core.take_standby());
        });
    }
}

/// Ends the running trial as reverted for `code`, from any thread.
fn end(control: &mut Control, code: u64, pid: u64, old: &mut Vec<Box<dyn SchedPolicy>>) {
    if control.active.take().is_none() {
        return;
    }
    if claim(code) {
        TRIP_PID.store(pid, Ordering::Relaxed);
    }
    ABORT.store(true, Ordering::Release);
    unstage_all(old);
    control.status.state = state::REVERTED;
    control.status.reason = TRIP_REASON.load(Ordering::Acquire);
    control.status.reason_pid = pid;
}

/// Wakes the driver so it sees the end at once.
pub(crate) fn wake_driver() {
    if let Some(tid) = DRIVER.load(Ordering::Acquire).checked_sub(1) {
        x86_64::instructions::interrupts::without_interrupts(|| threads::wake(tid));
    }
}

/// A person chose a policy: the running trial ends now.
pub(crate) fn cancel(control: &mut Control, old: &mut Vec<Box<dyn SchedPolicy>>) -> bool {
    let running = control.active.is_some();
    end(control, reason::CANCELLED, 0, old);
    if running {
        log!(
            "sched: trial {} cancelled by a policy switch",
            control.status.id
        );
    }
    running
}

pub(crate) fn on_process_exit(control: &mut Control, pid: u64, why: ExitReason) -> bool {
    let Some(active) = control.active.as_ref() else {
        return false;
    };
    let code = if pid == active.request.starter_pid {
        reason::STARTER_EXITED
    } else if pid == active.request.target_pid {
        reason::TARGET_EXITED
    } else if matches!(
        why,
        ExitReason::Runaway | ExitReason::Killed | ExitReason::Crashed
    ) {
        reason::KILLED
    } else {
        return false;
    };
    let mut old = Vec::new();
    end(control, code, pid, &mut old);
    log!(
        "sched: trial {} reverted at once: process {pid} {}",
        control.status.id,
        why.name()
    );
    // Dropping a policy frees memory, which is fine here: a thread, not a tick.
    drop(old);
    true
}

/// Starts a trial. Busy while one runs. Checks the plan's bounds; the system call
/// checks the spec, the labels and the target before.
pub fn start(request: Request) -> Result<u64, Error> {
    let window_ms = request.window_ns / 1_000_000;
    let pairs = request.pairs as u64;
    if !(kabi::sched::MIN_WINDOW_MS..=kabi::sched::MAX_WINDOW_MS).contains(&window_ms)
        || !(kabi::sched::MIN_PAIRS..=kabi::sched::MAX_PAIRS).contains(&pairs)
        || request.background.len() > kabi::sched::MAX_BACKGROUND
    {
        return Err(Error::Invalid);
    }
    let candidates: Vec<Box<dyn SchedPolicy>> = (0..smp::online())
        .map(|_| request.candidate.build())
        .collect::<Result<_, _>>()?;
    let table = class_table(&request.background);
    let mut old = Vec::new();
    let id = with(|control| {
        if control.active.is_some() {
            return Err(Error::Busy);
        }
        ABORT.store(false, Ordering::Release);
        TRIP_REASON.store(0, Ordering::Release);
        for (cpu, candidate) in candidates.into_iter().enumerate() {
            let staged = threads::with_core(cpu, |core, _| core.stage(candidate, table));
            match staged {
                Some(Ok(previous)) => old.extend(previous),
                _ => {
                    unstage_all(&mut old);
                    return Err(Error::Busy);
                }
            }
        }
        let (policy, preset) = match request.candidate {
            Candidate::Spec(spec) => ids(spec),
            Candidate::Test(_) => (kabi::policy::OTHER, 0),
        };
        let id = control.status.id + 1;
        control.status = kabi::sched::TrialStatus {
            id,
            state: state::RUNNING,
            policy,
            preset,
            target_pid: request.target_pid,
            windows_total: pairs * 2,
            window_ms,
            ..Default::default()
        };
        control.active = Some(Active {
            id,
            request: request.clone(),
        });
        Ok(id)
    });
    drop(old);
    let id = id?;
    if threads::spawn("sched-trial", drive, id as usize).is_err() {
        let mut old = Vec::new();
        with(|control| end(control, reason::NOT_RUN, 0, &mut old));
        return Err(Error::NoMemory);
    }
    Ok(id)
}

/// The processes a window measures: the target first, then every other.
fn processes(target: u64) -> Vec<u64> {
    let mut pids = vec![target];
    process::services(|pid, _| {
        if pid != target {
            pids.push(pid);
        }
    });
    pids
}

fn aborted() -> bool {
    ABORT.load(Ordering::Acquire)
}

/// `count` windows of `window_ns` of whatever policy runs, measured as a trial's
/// are, with `target_pid` as the target: the M10 exit test measures the default
/// and the candidate in separate boots this way. Also returns the processes in
/// the order the measures list them (target first) and the serial bytes sent
/// meanwhile; `None` if a trial or a person's switch got in the way.
pub fn measure_only(
    target_pid: u64,
    window_ns: u64,
    count: usize,
) -> Option<(Vec<WindowMeasure>, Vec<u64>, u64)> {
    let pids = processes(target_pid);
    let quiet_from = crate::serial::bytes_sent();
    let windows = (0..count)
        .map(|_| window(target_pid, window_ns, &pids))
        .collect::<Option<Vec<_>>>()?;
    Some((windows, pids, crate::serial::bytes_sent() - quiet_from))
}

/// Measures one window from now; `None` if the trial ended meanwhile.
fn window(target_pid: u64, window_ns: u64, pids: &[u64]) -> Option<WindowMeasure> {
    let start = hpet::nanos();
    for cpu in 0..smp::online() {
        threads::with_core(cpu, |core, _| core.begin_window());
    }
    let mut begin = Snapshots::new();
    begin.take();
    while hpet::nanos() < start + DISCARD_NS && !aborted() {
        threads::sleep_until_unless(start + DISCARD_NS, aborted);
    }
    let mut before = Snapshots::new();
    let mut after = Snapshots::new();
    before.take();
    let by_tid = process::pids_by_tid();
    let target = |tid: Tid| by_tid[tid] == target_pid;
    let mut drain = Drain::start();
    let mut samples = vec![Vec::new(); MAX_THREADS];
    let end = start + window_ns;
    if aborted() || !wait_draining(end, &mut drain, target, &mut samples, aborted) {
        return None;
    }
    after.take();
    let (sums, _) = after.since(&before);
    let by_tid = process::pids_by_tid();
    let per = |field: usize| -> Vec<u64> {
        pids.iter()
            .map(|&pid| {
                let threads = (0..MAX_THREADS).filter(|&tid| by_tid[tid] == pid);
                match field {
                    3 => threads.map(|tid| sums[tid][3]).max().unwrap_or(0),
                    _ => threads.map(|tid| sums[tid][field]).sum(),
                }
            })
            .collect()
    };
    let excess: Vec<u64> = samples
        .iter()
        .flatten()
        .map(|&ns| u64::from(ns).max(LATENCY_FLOOR_NS))
        .collect();
    let mut max_latency = vec![0; pids.len()];
    max_latency[0] = excess.iter().copied().max().unwrap_or(0);
    Some(WindowMeasure {
        from: before.cores.first().map_or(0, |s| s.at),
        to: after.cores.first().map_or(0, |s| s.at),
        target_excess_ns: excess,
        cpu_ns: per(0),
        max_wait_ns: per(3),
        max_latency_ns: max_latency,
        ejections: (after.ejections - begin.ejections) as u32,
    })
}

fn record(control: &mut Control, index: usize, measure: &WindowMeasure, candidate: bool) {
    let status = &mut control.status;
    if let Some(slot) = status.windows.get_mut(index) {
        *slot = WindowRecord {
            candidate: u64::from(candidate),
            target_p99_ns: measure.p99(),
            target_samples: measure.target_excess_ns.len() as u64,
            background_cpu_ns: measure.cpu_ns.iter().skip(1).sum(),
            max_wait_ns: measure.max_wait_ns.iter().copied().max().unwrap_or(0),
            ejections: u64::from(measure.ejections),
        };
    }
    status.windows_done = index as u64 + 1;
}

/// Flips every CPU to the candidate, or back, under the control lock; false if
/// the trial is no longer this one or a CPU refused.
fn switch_all(id: u64, to_candidate: bool) -> bool {
    with(|control| {
        if !mine(control, id) {
            return false;
        }
        if to_candidate {
            LAST_WAIT
                .iter()
                .for_each(|wait| wait.store(0, Ordering::Relaxed));
            ARMED.store(true, Ordering::Release);
        }
        let mut all = true;
        for cpu in 0..smp::online() {
            let done = threads::with_core(cpu, |core, now| match to_candidate {
                true => core.flip(now),
                false => core.revert(now) || !core.in_trial(),
            });
            all &= done == Some(true);
        }
        if !to_candidate {
            ARMED.store(false, Ordering::Release);
        }
        all
    })
}

/// The driver thread: the windows, the verdict and the ledger line.
fn drive(id: usize) {
    let id = id as u64;
    DRIVER.store(threads::current_tid() + 1, Ordering::Release);
    let Some(request) = with(|c| c.active.as_ref().map(|a| a.request.clone())) else {
        return;
    };
    let pids = processes(request.target_pid);
    let mut pairs = Vec::new();
    let mut ran = true;
    for pair in 0..request.pairs {
        let Some(baseline) = window(request.target_pid, request.window_ns, &pids) else {
            break;
        };
        with(|control| record(control, 2 * pair, &baseline, false));
        ran &= switch_all(id, true);
        let candidate = window(request.target_pid, request.window_ns, &pids);
        ran &= switch_all(id, false);
        let Some(candidate) = candidate else {
            break;
        };
        with(|control| record(control, 2 * pair + 1, &candidate, true));
        pairs.push(Pair {
            baseline,
            candidate,
        });
    }
    finish(id, &request, &pids, pairs, ran);
    DRIVER.store(0, Ordering::Release);
}

fn finish(id: u64, request: &Request, pids: &[u64], pairs: Vec<Pair>, ran: bool) {
    let complete = pairs.len() == request.pairs && !aborted();
    let mut verdict = sched::judge::judge(&pairs, 0);
    if !ran && complete {
        verdict.kept = false;
        verdict.reason = Some(Reason::NotRun);
    }
    let mut old = Vec::new();
    let line = with(|control| {
        if !mine(control, id) {
            return None;
        }
        if !complete {
            let trip = last_trip();
            let pid = match trip.reason {
                reason::GUARDRAIL => process::pids_by_tid()[trip.tid as usize % MAX_THREADS],
                _ => trip.pid,
            };
            let code = match trip.reason {
                reason::NONE => reason::NOT_RUN,
                code => code,
            };
            end(control, code, pid, &mut old);
            control.status.reason_value = trip.wait_ns;
            control.last_pairs = pairs;
            return Some(describe(control, request, pids));
        }
        let status = &mut control.status;
        status.baseline_p99_ns = verdict.baseline_p99_ns;
        status.candidate_p99_ns = verdict.candidate_p99_ns;
        status.background_permille = verdict.background_permille;
        if let Some((index, share)) = verdict.worst_process {
            status.worst_pid = pids.get(index).copied().unwrap_or(0);
            status.worst_permille = share;
        }
        let kept = verdict.kept && install(control, request, &mut old);
        if !kept {
            let (code, pid, value) = reason_code(verdict.reason, pids);
            end(control, code, pid, &mut old);
            control.status.reason_value = value;
        }
        control.last_pairs = pairs;
        Some(describe(control, request, pids))
    });
    drop(old);
    if let Some(line) = line {
        metrics::invalidate();
        log!("{line}");
    }
}

/// Puts the kept candidate in on every CPU; false (and nothing changed) if a CPU
/// refuses, such as one whose installed policy was ejected meanwhile.
fn install(control: &mut Control, request: &Request, old: &mut Vec<Box<dyn SchedPolicy>>) -> bool {
    let cpus = smp::online();
    let flipped = (0..cpus)
        .take_while(|&cpu| threads::with_core(cpu, |core, now| core.flip(now)) == Some(true))
        .count();
    if flipped < cpus {
        unstage_all(old);
        return false;
    }
    for cpu in 0..cpus {
        threads::with_core(cpu, |core, _| old.extend(core.commit()));
    }
    control.active = None;
    control.status.state = state::KEPT;
    if let Candidate::Spec(spec) = request.candidate {
        control.installed = spec;
        control.background = request.background.clone();
    }
    control.since_ns = hpet::nanos();
    true
}
