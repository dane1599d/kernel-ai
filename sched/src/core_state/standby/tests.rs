//! The standby slot, labels and policy hand-over. A counting allocator checks that
//! stage, flip, revert, commit and take_standby never allocate or free.

extern crate std;

use super::super::Core;
use crate::classes::{Class, ClassParams, Classes, MS};
use crate::{EnqueueReason, Interactive, MAX_THREADS, RoundRobin, SchedPolicy, State};
use alloc::boxed::Box;
use alloc::vec::Vec;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const WATCHDOG: u64 = 200 * MS;
const NORMAL: [Class; MAX_THREADS] = [Class::Normal; MAX_THREADS];

std::thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Counts allocations and frees made by a test thread while counting is on.
struct Counting;

fn count() {
    let _ = COUNTING.try_with(|on| {
        if on.get() {
            let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
        }
    });
}

// SAFETY: every call is forwarded unchanged to the system allocator; counting
// touches only const-initialized thread-locals, which never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: the caller's contract for `layout` is passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count();
        // SAFETY: `ptr` came from `System.alloc` with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many allocations and frees `f` made on this thread.
fn allocations(f: impl FnOnce()) -> usize {
    CALLS.with(|calls| calls.set(0));
    COUNTING.with(|on| on.set(true));
    f();
    COUNTING.with(|on| on.set(false));
    CALLS.with(Cell::get)
}

fn classes() -> Box<dyn SchedPolicy> {
    Box::new(Classes::new(ClassParams::new(4 * MS, 60 * MS).unwrap()))
}

fn busy_core() -> Core {
    let mut core = Core::new(Box::new(Interactive::new()), 0, WATCHDOG, 0);
    for tid in 3..7 {
        core.adopt(tid, "worker", EnqueueReason::New, 0);
    }
    assert_eq!(core.preempt(0), Some(3));
    core
}

#[test]
fn the_slot_never_allocates() {
    let mut core = busy_core();
    let mut labels = NORMAL;
    labels[4] = Class::Background;
    let (candidate, spare) = (classes(), classes());
    let mut dropped: Vec<Option<Box<dyn SchedPolicy>>> = Vec::with_capacity(4);
    let made = allocations(|| {
        dropped.push(core.stage(candidate, labels).ok().flatten());
        for round in 0..3u64 {
            let now = (10 + round * 2) * MS;
            assert!(core.flip(now));
            assert_eq!(core.policy_name(), "classes");
            assert_eq!(core.class(4), Class::Background);
            core.tick(now + MS);
            assert!(core.revert(now + MS));
            assert_eq!(core.policy_name(), "interactive");
            assert_eq!(core.class(4), Class::Normal);
        }
        // A guardrail's path: an ejection during a trial, then revert.
        assert!(core.flip(20 * MS));
        core.tick(20 * MS + WATCHDOG + MS);
        assert!(core.on_fallback());
        assert!(core.revert(20 * MS + WATCHDOG + MS));
        dropped.push(core.take_standby());
        dropped.push(core.stage(spare, labels).ok().flatten());
        assert!(core.flip(500 * MS));
        dropped.push(core.commit());
    });
    assert_eq!(made, 0, "the standby slot allocated or freed");
    let names: Vec<_> = dropped
        .iter()
        .map(|p| p.as_ref().map(|p| p.name()))
        .collect();
    assert_eq!(names, [None, Some("classes"), None, Some("interactive")]);
    assert!(!core.in_trial());
    assert_eq!(core.class(4), Class::Background);
}

#[test]
fn the_slot_refuses_what_does_not_fit_the_trial_state() {
    let mut core = busy_core();
    assert!(!core.flip(MS), "nothing staged");
    assert!(!core.revert(MS), "no trial");
    assert!(core.commit().is_none());
    assert!(core.stage(classes(), NORMAL).is_ok_and(|old| old.is_none()));
    let earlier = core.stage(Box::new(RoundRobin::new()), NORMAL);
    assert_eq!(earlier.ok().flatten().map(|p| p.name()), Some("classes"));
    assert_eq!(core.standby_name(), Some("round-robin"));
    assert!(core.flip(MS));
    assert!(!core.flip(MS), "a trial already runs");
    let refused = core.stage(classes(), NORMAL);
    assert_eq!(refused.err().map(|p| p.name()), Some("classes"));
    assert!(
        core.take_standby().is_none(),
        "the last good policy stays during a trial"
    );
    assert!(core.revert(2 * MS));
    // Reverted, the candidate stays staged and can run again.
    assert!(core.flip(3 * MS));
    assert!(core.revert(4 * MS));
    assert_eq!(core.take_standby().map(|p| p.name()), Some("round-robin"));
    assert!(!core.flip(5 * MS), "nothing staged any more");
}

#[test]
fn an_ejected_candidate_cannot_be_committed_only_reverted() {
    let mut core = busy_core();
    let _ = core.stage(Box::new(RoundRobin::new()), NORMAL);
    assert!(core.flip(0));
    core.tick(WATCHDOG + MS);
    assert!(core.on_fallback());
    assert!(core.commit().is_none());
    assert!(core.in_trial());
    assert!(core.revert(WATCHDOG + MS));
    assert!(!core.on_fallback());
    assert_eq!(core.policy_name(), "interactive");
}

#[test]
fn an_ejected_policy_is_not_a_last_good_one_to_return_to() {
    let mut core = busy_core();
    core.tick(WATCHDOG + MS);
    assert!(core.on_fallback());
    let _ = core.stage(classes(), NORMAL);
    assert!(!core.flip(WATCHDOG + 2 * MS));
}

#[test]
fn the_restored_policy_is_judged_only_on_its_own_waits() {
    let mut core = busy_core();
    let _ = core.stage(Box::new(RoundRobin::new()), NORMAL);
    assert!(core.flip(0));
    // Three threads wait past the watchdog under the candidate.
    core.tick(WATCHDOG + MS);
    assert!(core.on_fallback());
    assert!(core.revert(WATCHDOG + MS));
    // The next ticks do not eject Interactive for waits the candidate caused.
    for step in 2..20 {
        core.tick(WATCHDOG + step * MS);
    }
    assert!(!core.on_fallback());
    assert_eq!(core.ejections(), 1);
}

#[test]
fn a_person_choosing_a_policy_ends_the_trial_with_the_old_labels() {
    let mut core = busy_core();
    let mut labels = NORMAL;
    labels[5] = Class::Background;
    let _ = core.stage(classes(), labels);
    assert!(core.flip(MS));
    assert_eq!(core.class(5), Class::Background);
    let old = core.set_policy(Box::new(RoundRobin::new()));
    assert_eq!(old.name(), "classes");
    assert!(!core.in_trial());
    assert!(!core.revert(2 * MS));
    assert_eq!(core.policy_name(), "round-robin");
    assert_eq!(
        core.class(5),
        Class::Normal,
        "the trial's labels went with it"
    );
    assert!(
        !core.flip(3 * MS),
        "the old policy left in the slot is not a candidate"
    );
    assert_eq!(core.take_standby().map(|p| p.name()), Some("interactive"));
}

#[test]
fn a_label_set_during_a_trial_belongs_to_the_candidate() {
    let mut core = busy_core();
    let _ = core.stage(classes(), NORMAL);
    assert!(core.flip(MS));
    core.set_class(6, Class::Background);
    assert!(core.revert(2 * MS));
    assert_eq!(core.class(6), Class::Normal);
    assert!(core.flip(3 * MS));
    assert_eq!(core.class(6), Class::Background);
}

#[test]
fn a_switch_keeps_the_boost_of_a_thread_that_woke_and_has_not_run() {
    let mut core = Core::new(Box::new(Interactive::new()), 0, WATCHDOG, 0);
    core.adopt(3, "worker", EnqueueReason::New, 0);
    core.adopt(4, "typist", EnqueueReason::New, 0);
    core.adopt(5, "busy", EnqueueReason::New, 0);
    assert_eq!(core.preempt(0), Some(3));
    assert_eq!(core.preempt(MS), Some(4));
    assert_eq!(core.block_current(MS), 5);
    assert_eq!(core.block_current(MS), 3);
    assert!(core.wake(5, 2 * MS));
    assert_eq!(core.preempt_if_pending(2 * MS), Some(5));
    // The typist wakes behind a boosted thread, so it waits, after the worker
    // that has been queued since 1 ms.
    assert!(!core.wake(4, 3 * MS));
    let _ = core.set_policy(Box::new(Interactive::new()));
    // The running thread that woke keeps its shield...
    core.adopt(7, "other", EnqueueReason::New, 3 * MS);
    assert!(core.state(5) == State::Running);
    // ...and, handed over as woken, the typist still goes first.
    assert_eq!(core.block_current(4 * MS), 4);
}

#[test]
fn the_running_thread_that_woke_stays_shielded_across_a_switch() {
    let mut core = Core::new(Box::new(Interactive::new()), 0, WATCHDOG, 0);
    core.adopt(3, "hog", EnqueueReason::New, 0);
    core.adopt(4, "typist", EnqueueReason::New, 0);
    core.adopt(5, "sleeper", EnqueueReason::New, 0);
    assert_eq!(core.preempt(0), Some(3));
    assert_eq!(core.preempt(MS), Some(4));
    assert_eq!(core.block_current(MS), 5);
    assert_eq!(core.block_current(MS), 3);
    assert!(core.wake(4, 2 * MS));
    assert_eq!(core.preempt_if_pending(2 * MS), Some(4));
    let _ = core.stage(Box::new(Interactive::new()), NORMAL);
    assert!(core.flip(2 * MS));
    // The typist woke and is running: another wake does not cut it short.
    assert!(!core.wake(5, 3 * MS));
    assert_eq!(core.current(), 4);
}

#[test]
fn labels_are_replayed_into_every_new_policy_and_a_new_thread_starts_normal() {
    let mut core = Core::new(Box::new(Interactive::new()), 0, WATCHDOG, 0);
    core.adopt(3, "hog", EnqueueReason::New, 0);
    core.adopt(4, "build", EnqueueReason::New, 0);
    core.set_class(4, Class::Background);
    core.set_class(MAX_THREADS, Class::Background);
    let _ = core.set_policy(classes());
    assert_eq!(core.preempt(0), Some(3));
    assert_eq!(core.preempt(MS), Some(4));
    assert_eq!(core.block_current(MS), 3);
    // A background thread's wake never preempts.
    assert!(!core.wake(4, 2 * MS));
    assert_eq!(core.class(4), Class::Background);
    assert_eq!(core.class(MAX_THREADS), Class::Normal);
    // The thread exits; a new one in its slot is not labelled.
    assert_eq!(core.preempt(3 * MS), Some(4));
    assert_eq!(core.exit_current(4 * MS), 3);
    assert!(core.release(4));
    core.adopt(4, "new", EnqueueReason::New, 4 * MS);
    assert_eq!(core.class(4), Class::Normal);
}
