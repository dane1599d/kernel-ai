//! What the shell will see in the OS, measured here in the simulator: one 3 s
//! window of the mix under Interactive, after a 1 s warm-up, turned into the
//! summary's rows from the Core's per-thread accounting.

use llmproto::tune::{Label, Observation, ProcessRow};
use sched::sim::{MS, Sim};
use sched::spec::PolicySpec;
use sched::stats::{BUCKETS, ThreadWindow, percentile};
use sched::workload::Mix;
use sched::{IDLE, Interactive};

pub const WARMUP_NS: u64 = 1_000 * MS;
pub const WINDOW_NS: u64 = 3_000 * MS;

/// The `p`th percentile of a window's wake-to-run times, in microseconds: exact
/// when the ring still holds every sample, else the upper edge of the log2 bucket
/// it falls in, but never above the window's maximum.
pub(super) fn wait_us(sim: &Sim, tid: usize, window: &ThreadWindow, p: u64) -> u64 {
    let count = window.sample_count() as usize;
    if count == 0 {
        return 0;
    }
    let mut exact = vec![0u32; count];
    let stats = sim.core().stats();
    if let Some(n) = stats.samples(tid, window.first_sample, window.end_sample, &mut exact) {
        let mut sorted: Vec<u64> = exact[..n].iter().map(|&ns| u64::from(ns)).collect();
        sorted.sort_unstable();
        return percentile(&sorted, p) / 1000;
    }
    let rank = (p * count as u64).div_ceil(100).max(1);
    let mut seen = 0u64;
    for (bucket, &n) in window.hist.iter().enumerate().take(BUCKETS) {
        seen += u64::from(n);
        if seen >= rank {
            let edge = 1u64.checked_shl(bucket as u32 + 1).unwrap_or(u64::MAX);
            return edge.min(window.max_latency_ns) / 1000;
        }
    }
    window.max_latency_ns / 1000
}

/// Observes `mix` with `seed` and returns what the summary is built from.
pub fn observe(mix: &Mix, seed: u64, intent: Option<String>) -> Observation {
    let mut sim = Sim::new(mix.procs, seed, Box::new(Interactive::new()));
    sim.run_until(WARMUP_NS);
    sim.core_mut().begin_window();
    let start = sim.core().snapshot(WARMUP_NS);
    sim.run_until(WARMUP_NS + WINDOW_NS);
    let window = sim.core().snapshot(WARMUP_NS + WINDOW_NS).since(&start);
    let processes = mix
        .procs
        .iter()
        .enumerate()
        .map(|(index, proc)| {
            let tid = sim.tid(index).expect("every process of the mix runs");
            let t = &window.threads[tid];
            ProcessRow {
                name: proc.name.to_string(),
                threads: 1,
                cpu_permille: t.cpu_ns * 1000 / WINDOW_NS,
                wakes_per_s: t.wakes * 1_000_000_000 / WINDOW_NS,
                // CPU per wake, across preemptions, as the burst rule counts a
                // burst; a thread that never slept ran one burst all window.
                mean_burst_us: t.cpu_ns / t.wakes.max(1) / 1000,
                wait_p50_us: wait_us(&sim, tid, t, 50),
                wait_p99_us: wait_us(&sim, tid, t, 99),
                wait_max_us: t.max_latency_ns / 1000,
                label: Label::Normal,
                protected: false,
            }
        })
        .collect();
    let idle = window.threads[IDLE].cpu_ns;
    Observation {
        target: mix.target,
        intent,
        current: PolicySpec::DEFAULT,
        since_s: (WARMUP_NS + WINDOW_NS) / 1_000_000_000,
        verdicts: Vec::new(),
        cpus: 1,
        busy_permille: (WINDOW_NS - idle.min(WINDOW_NS)) * 1000 / WINDOW_NS,
        window_ms: WINDOW_NS / MS,
        processes,
    }
}
