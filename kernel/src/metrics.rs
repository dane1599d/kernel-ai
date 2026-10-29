//! Read-only system numbers for ring 3 (`kabi::Metrics`), from a snapshot at most
//! [`MAX_AGE_NS`] old, so a program calling it in a loop cannot keep taking the
//! scheduler, memory and process-table locks.

use crate::sched_control::policy_id;
use crate::{hpet, memory, process, smp, threads};
use kabi::{MAX_METRIC_PROCESSES, Metrics};
use spin::Mutex;

const MAX_AGE_NS: u64 = 10_000_000;

static CACHE: Mutex<Option<(u64, Metrics)>> = Mutex::new(None);

fn take() -> Metrics {
    let mut m = Metrics::empty();
    m.cpus = smp::online() as u64;
    let (running, ejections, switches) = threads::stats();
    m.running_policy = policy_id(running);
    m.policy = policy_id(threads::installed_policy_name());
    m.ejections = u64::from(ejections);
    m.context_switches = switches;
    m.migrations = threads::migrations();
    let (free, total) = memory::with(|mem| (mem.free_frames(), mem.total_frames()));
    m.free_frames = free;
    m.total_frames = total;
    let (allocations, bytes) = memory::heap::live();
    m.heap_allocations = allocations as u64;
    m.heap_bytes = bytes as u64;
    m.uptime_ns = hpet::nanos();
    process::services_usage(|pid, name, pages, cpu_ns| {
        let index = m.process_count as usize;
        if index < MAX_METRIC_PROCESSES {
            let (bytes, len) = name.bytes();
            m.processes[index].pid = pid;
            m.processes[index].name = bytes;
            m.processes[index].name_len = len as u64;
            m.processes[index].pages = pages;
            m.processes[index].cpu_ns = cpu_ns;
            m.process_count += 1;
        } else {
            m.processes_omitted += 1;
        }
    });
    m
}

/// Drops the snapshot, so a change (such as a policy switch) shows at once.
pub fn invalidate() {
    threads::preempt_disable();
    *CACHE.lock() = None;
    threads::preempt_enable();
}

/// The current numbers. Must be called with interrupts on.
pub fn snapshot() -> Metrics {
    let now = hpet::nanos();
    threads::preempt_disable();
    let cached = CACHE
        .lock()
        .filter(|(at, _)| now.saturating_sub(*at) < MAX_AGE_NS)
        .map(|(_, m)| m);
    threads::preempt_enable();
    if let Some(metrics) = cached {
        return metrics;
    }
    let fresh = take();
    threads::preempt_disable();
    *CACHE.lock() = Some((now, fresh));
    threads::preempt_enable();
    fresh
}
