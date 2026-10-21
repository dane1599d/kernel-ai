//! Property tests over random histories from a seeded generator (no proptest:
//! no new crates).

use super::*;
use crate::Interactive;
use alloc::vec::Vec;

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) % n.max(1)
    }
}

const THREADS: usize = 12;
const HISTORIES: u64 = 3_000;

fn params(normal_slice_ns: u64, max_wait_ns: u64) -> ClassParams {
    ClassParams::new(normal_slice_ns, max_wait_ns).unwrap()
}

fn reason(rng: &mut Rng) -> EnqueueReason {
    match rng.below(4) {
        0 => EnqueueReason::New,
        1 => EnqueueReason::Preempted,
        2 => EnqueueReason::Yielded,
        _ => EnqueueReason::Woken,
    }
}

/// Feeds both policies the same valid calls (as Core would make them) and
/// returns false at the first answer that differs. `compare_ticks` is off when
/// the normal slices differ, since only the slice length may then differ.
fn same_answers(
    a: &mut dyn SchedPolicy,
    b: &mut dyn SchedPolicy,
    seed: u64,
    compare_ticks: bool,
) -> bool {
    let mut rng = Rng::new(seed);
    let mut queued = [false; THREADS];
    let mut current = 0;
    for _ in 0..300 {
        let tid = 1 + rng.below(THREADS as u64 - 1) as usize;
        match rng.below(5) {
            0 | 1 if !queued[tid] && tid != current => {
                let why = reason(&mut rng);
                a.enqueue(tid, why);
                b.enqueue(tid, why);
                queued[tid] = true;
                let other = 1 + rng.below(THREADS as u64 - 1) as usize;
                if a.preempt_on_wake(tid, other) != b.preempt_on_wake(tid, other) {
                    return false;
                }
            }
            2 if queued[tid] => {
                a.dequeue(tid);
                b.dequeue(tid);
                queued[tid] = false;
            }
            3 => {
                let picked = a.pick_next();
                if picked != b.pick_next() {
                    return false;
                }
                if let Some(next) = picked {
                    queued[next] = false;
                    current = next;
                }
            }
            _ => {
                let slice = rng.below(15 * MS);
                let (x, y) = (a.tick(current, slice), b.tick(current, slice));
                if compare_ticks && x != y {
                    return false;
                }
            }
        }
    }
    true
}

#[test]
fn classes_with_every_thread_normal_answers_exactly_as_interactive() {
    for seed in 0..HISTORIES {
        let mut classes = Classes::new(params(INTERACTIVE_NORMAL_SLICE_NS, 20 * MS));
        let mut interactive = Interactive::new();
        assert!(
            same_answers(&mut classes, &mut interactive, seed, true),
            "seed {seed}"
        );
    }
}

#[test]
fn with_a_preset_and_no_labels_only_the_slice_length_differs() {
    for seed in 0..HISTORIES {
        let mut classes = Classes::new(params(4 * MS, 60 * MS));
        let mut interactive = Interactive::new();
        assert!(
            same_answers(&mut classes, &mut interactive, seed, false),
            "seed {seed}"
        );
    }
}

/// Pick order when both policies are drained after the same history.
fn drain(policy: &mut dyn SchedPolicy) -> Vec<Tid> {
    core::iter::from_fn(|| policy.pick_next()).collect()
}

#[test]
fn a_label_never_moves_a_thread_ahead_of_where_interactive_puts_it() {
    for seed in 0..HISTORIES {
        let mut rng = Rng::new(seed ^ 0xabcd);
        let mut classes = Classes::new(params(4 * MS, 10 * MS));
        let mut interactive = Interactive::new();
        let labelled: [bool; THREADS] = core::array::from_fn(|_| rng.below(2) == 0);
        let mut queued = [false; THREADS];
        for (tid, &label) in labelled.iter().enumerate() {
            if label && rng.below(4) != 0 {
                classes.set_class(tid, Class::Background);
            }
        }
        for _ in 0..80 {
            let tid = rng.below(THREADS as u64) as usize;
            match rng.below(6) {
                0..=2 if !queued[tid] => {
                    let why = reason(&mut rng);
                    classes.enqueue(tid, why);
                    interactive.enqueue(tid, why);
                    queued[tid] = true;
                }
                3 if queued[tid] => {
                    classes.dequeue(tid);
                    interactive.dequeue(tid);
                    queued[tid] = false;
                }
                // Labels may also arrive while threads are queued.
                4 if labelled[tid] => classes.set_class(tid, Class::Background),
                _ => {
                    for _ in 0..rng.below(8) {
                        classes.tick(0, 0);
                        interactive.tick(0, 0);
                    }
                }
            }
        }
        let labels = classes.background;
        let is_labelled = |tid: Tid| labels[tid];
        let (order_i, order_c) = (drain(&mut interactive), drain(&mut classes));
        let mut sorted_i = order_i.clone();
        let mut sorted_c = order_c.clone();
        sorted_i.sort_unstable();
        sorted_c.sort_unstable();
        assert_eq!(
            sorted_i, sorted_c,
            "seed {seed}: both hold the same threads"
        );
        let position = |order: &[Tid], tid: Tid| order.iter().position(|&t| t == tid).unwrap();
        for &x in order_i.iter().filter(|&&x| !is_labelled(x)) {
            for &y in &order_i {
                if position(&order_i, x) < position(&order_i, y) {
                    assert!(
                        position(&order_c, x) < position(&order_c, y),
                        "seed {seed}: unlabelled {x} ran before {y} under Interactive \
                         but not under classes: {order_i:?} vs {order_c:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_background_wake_goes_after_unlabelled_wakes_and_before_the_normal_ring() {
    let mut policy = Classes::new(params(6 * MS, 20 * MS));
    policy.set_class(3, Class::Background);
    policy.enqueue(1, EnqueueReason::New);
    assert_eq!(policy.pick_next(), Some(1));
    policy.enqueue(2, EnqueueReason::New);
    policy.enqueue(3, EnqueueReason::Woken);
    // It still cuts short a thread Interactive would let it cut short (a hog)...
    assert!(policy.preempt_on_wake(3, 1));
    policy.enqueue(4, EnqueueReason::Woken);
    assert!(policy.preempt_on_wake(4, 1));
    // ...but the unlabelled wake goes first; the never-woken thread goes last.
    assert_eq!(drain(&mut policy), [4, 3, 2]);
}

#[test]
fn a_background_thread_that_waited_past_the_maximum_goes_before_later_wakes() {
    let mut policy = Classes::new(params(6 * MS, 20 * MS));
    policy.set_class(3, Class::Background);
    policy.enqueue(1, EnqueueReason::New);
    assert_eq!(policy.pick_next(), Some(1));
    policy.enqueue(2, EnqueueReason::Preempted);
    policy.enqueue(3, EnqueueReason::Woken);
    for _ in 0..20 {
        assert!(!policy.tick(1, MS));
    }
    // Past 20 ms thread 3 is due: the tick preempts the unboosted runner...
    assert!(policy.tick(1, 2 * MS));
    policy.enqueue(1, EnqueueReason::Preempted);
    policy.enqueue(5, EnqueueReason::Woken);
    // ...and it goes before thread 2 and before the wake that came after it.
    assert_eq!(policy.pick_next(), Some(3));
    // It runs a boosted-length slice, and is not preempted for another due thread.
    assert!(!policy.tick(3, MS));
    assert!(policy.tick(3, BOOSTED_SLICE_NS));
    assert_eq!(drain(&mut policy), [5, 2, 1]);
}

#[test]
fn a_due_thread_waits_for_wakes_that_came_before_it() {
    let mut policy = Classes::new(params(6 * MS, 10 * MS));
    policy.set_class(3, Class::Background);
    policy.enqueue(7, EnqueueReason::Woken);
    policy.enqueue(3, EnqueueReason::Woken);
    for _ in 0..11 {
        policy.tick(1, 0);
    }
    assert_eq!(drain(&mut policy), [7, 3]);
}

#[test]
fn a_label_moves_a_queued_boosted_thread_behind_unlabelled_wakes_only() {
    let mut policy = Classes::new(params(6 * MS, 20 * MS));
    policy.enqueue(2, EnqueueReason::New);
    policy.enqueue(3, EnqueueReason::Woken);
    policy.enqueue(4, EnqueueReason::Woken);
    policy.set_class(3, Class::Background);
    assert_eq!(drain(&mut policy), [4, 3, 2]);
    policy.set_class(MAX_THREADS, Class::Background);
}

#[test]
fn responsive_boosts_only_short_bursts() {
    let mut policy = Responsive::new();
    // Thread 2 ran 3 ms (seen across a preemption) before it slept; thread 3 ran
    // under a millisecond.
    policy.enqueue(2, EnqueueReason::New);
    assert_eq!(policy.pick_next(), Some(2));
    policy.tick(2, 2 * MS);
    policy.enqueue(2, EnqueueReason::Preempted);
    assert_eq!(policy.pick_next(), Some(2));
    policy.tick(2, MS);
    policy.enqueue(3, EnqueueReason::New);
    assert_eq!(policy.pick_next(), Some(3));
    policy.tick(3, MS / 2);
    policy.enqueue(1, EnqueueReason::New);
    assert_eq!(policy.pick_next(), Some(1));
    policy.enqueue(2, EnqueueReason::Woken);
    policy.enqueue(3, EnqueueReason::Woken);
    assert!(!policy.preempt_on_wake(2, 1));
    assert!(policy.preempt_on_wake(3, 1));
    assert_eq!(drain(&mut policy), [3, 2]);
}

#[test]
fn a_label_never_lets_a_thread_preempt_where_interactive_would_not() {
    for seed in 0..HISTORIES {
        let mut rng = Rng::new(seed ^ 0x5eed);
        let mut classes = Classes::new(params(6 * MS, 20 * MS));
        let mut interactive = Interactive::new();
        for tid in 0..THREADS {
            if rng.below(2) == 0 {
                classes.set_class(tid, Class::Background);
            }
        }
        // The running thread: queued for some reason, then taken off the queue,
        // which leaves the flags its last enqueue set, as a pick does.
        let current = rng.below(THREADS as u64) as usize;
        let why = reason(&mut rng);
        classes.enqueue(current, why);
        interactive.enqueue(current, why);
        classes.dequeue(current);
        interactive.dequeue(current);
        for _ in 0..rng.below(30) {
            classes.tick(current, 0);
        }
        let woken = rng.below(THREADS as u64) as usize;
        if woken != current {
            classes.enqueue(woken, EnqueueReason::Woken);
            interactive.enqueue(woken, EnqueueReason::Woken);
        }
        if classes.preempt_on_wake(woken, current) {
            assert!(interactive.preempt_on_wake(woken, current), "seed {seed}");
        }
        // For an unlabelled wake, the answers are the same.
        if !classes.background[woken] {
            assert_eq!(
                classes.preempt_on_wake(woken, current),
                interactive.preempt_on_wake(woken, current),
                "seed {seed}"
            );
        }
    }
}

#[test]
fn a_background_thread_that_woke_keeps_interactives_shield_once_it_runs() {
    let mut policy = Classes::new(params(6 * MS, 60 * MS));
    policy.set_class(3, Class::Background);
    policy.enqueue(3, EnqueueReason::Woken);
    assert_eq!(policy.pick_next(), Some(3));
    policy.enqueue(4, EnqueueReason::Woken);
    // Like a boosted thread under Interactive: no preemption on wake, and the
    // short slice.
    assert!(!policy.preempt_on_wake(4, 3));
    assert!(!policy.tick(3, MS));
    assert!(policy.tick(3, BOOSTED_SLICE_NS));
    // Once preempted it is an ordinary normal-ring thread.
    policy.enqueue(3, EnqueueReason::Preempted);
    assert_eq!(policy.pick_next(), Some(4));
    policy.enqueue(4, EnqueueReason::Preempted);
    assert_eq!(policy.pick_next(), Some(3));
    policy.enqueue(5, EnqueueReason::Woken);
    assert!(policy.preempt_on_wake(5, 3));
}

#[test]
fn removing_a_label_gives_back_what_interactive_gives() {
    let mut policy = Classes::new(params(6 * MS, 20 * MS));
    policy.set_class(3, Class::Background);
    policy.enqueue(2, EnqueueReason::New);
    policy.enqueue(4, EnqueueReason::Woken);
    policy.enqueue(3, EnqueueReason::Woken);
    policy.enqueue(5, EnqueueReason::Woken);
    // Unlabelled again while queued: back in the woken ring, in its place.
    policy.set_class(3, Class::Normal);
    assert_eq!(drain(&mut policy), [4, 3, 5, 2]);
    // Unlabelled while running: shielded as a woken thread is.
    policy.set_class(6, Class::Background);
    policy.enqueue(6, EnqueueReason::Woken);
    assert_eq!(policy.pick_next(), Some(6));
    policy.set_class(6, Class::Normal);
    policy.enqueue(7, EnqueueReason::Woken);
    assert!(!policy.preempt_on_wake(7, 6));
}

#[test]
fn the_oldest_due_thread_goes_first_even_out_of_ring_order() {
    let mut policy = Classes::new(params(6 * MS, 10 * MS));
    policy.set_class(3, Class::Background);
    policy.enqueue(1, EnqueueReason::Woken);
    policy.enqueue(2, EnqueueReason::Woken);
    policy.enqueue(3, EnqueueReason::Woken);
    // Labelled while queued, thread 1 joins the normal ring in queueing order.
    policy.set_class(1, Class::Background);
    for _ in 0..11 {
        policy.tick(9, 0);
    }
    assert_eq!(drain(&mut policy), [1, 2, 3]);
}
