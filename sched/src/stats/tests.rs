use super::*;

#[test]
fn buckets_are_powers_of_two() {
    assert_eq!(bucket(0), 0);
    assert_eq!(bucket(1), 0);
    assert_eq!(bucket(2), 1);
    assert_eq!(bucket(3), 1);
    assert_eq!(bucket(1_000_000), 19);
    assert_eq!(bucket(u64::MAX), BUCKETS - 1);
}

#[test]
fn percentile_is_nearest_rank() {
    let sorted: Vec<u64> = (1..=100).collect();
    assert_eq!(percentile(&sorted, 99), 99);
    assert_eq!(percentile(&sorted, 50), 50);
    assert_eq!(percentile(&sorted, 100), 100);
    assert_eq!(percentile(&[7], 99), 7);
    assert_eq!(percentile(&[], 99), 0);
    // Three samples: p99 is the largest.
    assert_eq!(percentile(&[1, 2, 3], 99), 3);
}

#[test]
fn only_woken_runs_are_latency_samples() {
    let mut stats = Stats::new();
    stats.runnable(5, EnqueueReason::New);
    stats.picked(5, 4_000);
    stats.runnable(5, EnqueueReason::Woken);
    stats.picked(5, 1_500);
    stats.runnable(5, EnqueueReason::Preempted);
    stats.preempted(5);
    stats.picked(5, 9_000);
    let c = stats.counters(5);
    assert_eq!((c.wakes, c.preemptions, c.samples), (1, 1, 1));
    assert_eq!(c.latency_sum_ns, 1_500);
    assert_eq!(c.window_max_wait_ns, 9_000);
    assert_eq!(c.window_max_latency_ns, 1_500);
    assert_eq!(c.hist[bucket(1_500)], 1);
    let mut out = [0u32; 4];
    assert_eq!(stats.samples(5, 0, 1, &mut out), Some(1));
    assert_eq!(out[0], 1_500);
    stats.begin_window();
    assert_eq!(stats.counters(5).window_max_wait_ns, 0);
    // Out-of-range thread IDs are ignored.
    stats.runnable(MAX_THREADS, EnqueueReason::Woken);
    assert_eq!(stats.counters(MAX_THREADS).wakes, 0);
}

#[test]
fn the_ring_keeps_the_last_samples_and_reports_lost_ones() {
    let mut stats = Stats::new();
    let total = SAMPLES as u64 + 10;
    for i in 0..total {
        stats.runnable(3, EnqueueReason::Woken);
        stats.picked(3, i);
    }
    let mut out = [0u32; SAMPLES];
    // The first ten were overwritten.
    assert_eq!(stats.samples(3, 0, total, &mut out), None);
    assert_eq!(stats.samples(3, 9, total, &mut out), None);
    assert_eq!(stats.samples(3, 10, total, &mut out), Some(SAMPLES));
    assert_eq!(out[0], 10);
    assert_eq!(out[SAMPLES - 1], (total - 1) as u32);
    // Not yet recorded, or backwards.
    assert_eq!(stats.samples(3, 10, total + 1, &mut out), None);
    assert_eq!(stats.samples(3, 20, 10, &mut out), None);
    // Too small a buffer.
    assert_eq!(stats.samples(3, 100, 110, &mut [0u32; 5]), None);
}

#[test]
fn windows_subtract_counters_and_keep_the_later_maxima() {
    let mut stats = Stats::new();
    stats.ran(2, 1_000);
    stats.runnable(2, EnqueueReason::Woken);
    stats.picked(2, 50);
    let earlier = Snapshot {
        at: 100,
        threads: core::array::from_fn(|tid| stats.counters(tid)),
    };
    stats.begin_window();
    let reset = stats.counters(2);
    assert_eq!(
        (reset.window_max_wait_ns, reset.window_max_latency_ns),
        (0, 0)
    );
    stats.ran(2, 4_000);
    stats.runnable(2, EnqueueReason::Woken);
    stats.picked(2, 30);
    let later = Snapshot {
        at: 5_100,
        threads: core::array::from_fn(|tid| stats.counters(tid)),
    };
    let window = later.since(&earlier);
    assert_eq!(window.ns, 5_000);
    let t = window.threads[2];
    assert_eq!((t.cpu_ns, t.wakes), (4_000, 1));
    assert_eq!((t.first_sample, t.end_sample, t.sample_count()), (1, 2, 1));
    // The 50 ns before the window does not count; only the 30 ns inside it.
    assert_eq!((t.max_wait_ns, t.max_latency_ns), (30, 30));
    assert_eq!(t.hist.iter().sum::<u32>(), 1);
}

#[test]
fn snapshots_of_two_cores_add_counters_and_keep_the_larger_maxima() {
    let mut a = Snapshot::EMPTY;
    let mut b = Snapshot::EMPTY;
    a.threads[4].cpu_ns = 10;
    a.threads[4].window_max_wait_ns = 7;
    a.threads[4].hist[3] = u32::MAX;
    b.threads[4].cpu_ns = 5;
    b.threads[4].window_max_wait_ns = 9;
    b.threads[4].hist[3] = 2;
    a.absorb(&b);
    assert_eq!(a.threads[4].cpu_ns, 15);
    assert_eq!(a.threads[4].window_max_wait_ns, 9);
    // Buckets wrap rather than overflow, and a window across the wrap is right.
    assert_eq!(a.threads[4].hist[3], 1);
    let mut earlier = Snapshot::EMPTY;
    earlier.threads[4].hist[3] = u32::MAX;
    assert_eq!(a.since(&earlier).threads[4].hist[3], 2);
}
