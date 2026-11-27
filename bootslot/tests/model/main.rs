//! `bootslot`'s transitions checked exhaustively (plan P6 G46, ADR 0008's
//! review, parts 2 and 3): every sequence of up to [`DEPTH`] boots from each
//! starting record, where each boot may lose power, or fault, after any one of
//! its steps, and a firmware write may fail. The steps are the kernel's
//! (`kernel/src/slots.rs`), driven here through the same `bootslot` functions
//! the kernel calls ([`kernel::REAL`]), with closures that record each step:
//!
//! - a system slot's *up*: [`bootslot::up_writes`], the fault path told the
//!   new record, then the record, then the one-shot;
//! - a `BOOTREC_SET` (a commit, a stage, a rollback, a recovery request):
//!   refused as [`bootslot::may_set`] says, over the record the firmware holds
//!   then, else [`bootslot::set_writes`]; `updated` sets once more, rebased
//!   ([`bootslot::rebase`]), after a Busy. Probation's commit may land between
//!   any read `updated` made and each of its sets (`plans.rs`);
//! - the fault path: [`bootslot::fault_writes`] on the record it was last
//!   told;
//! - R: [`bootslot::arm_writes`], or the record alone when it stays.
//!
//! The firmware only takes the one-shot (Limine erases it); the kernel never
//! runs [`bootslot::boot`]. After each write the walk checks:
//!
//! 1. the floors never fall, and rise only in a commit's record, to the
//!    release of the slot that committed;
//! 2. no committed slot is below the floors or retired;
//! 3. a committed slot stays committed, at its release, unless a stage
//!    begins over it or the other slot's commit retires it below the floors,
//!    and once a slot is committed one always is;
//! 4. `came_up` is set only by the armed slot's *up*, and kept only while the
//!    armed slot stays the same;
//!
//! and at each boot:
//!
//! 5. the boot is one [`bootslot::may_boot`] allows: R, a committed slot, or
//!    a trial slot with tries left;
//! 6. a trial slot boots no more often than its tries allow before R or a
//!    committed slot boots instead (see [`walk::budget`]);
//! 7. R stays up only when asked to, or when no committed slot is left that
//!    it could fall back to;
//! 8. a boot spared its try (R stored the one-shot and not the record) only
//!    after a power cut between R's writes, never after a write that failed:
//!    otherwise a store that refuses every record would boot the trial for
//!    ever.
//!
//! And after each commit's write:
//!
//! 9. the record is the commit of the record the firmware held, so a
//!    BOOTREC_SET that landed before it is kept (probation holds SETTING
//!    from its read to its set, and `updated`'s set racing it waits).
//!
//! The kernel's glue around these steps ([`bootslot::glue`]: what
//! BOOTREC_SET hands `may_set`, what *up* keeps when its record is lost, and
//! the commit's SETTING) is driven too, through the same functions.
//!
//! `tests/mutations.rs` runs the same walk with each of the kernel's fixes
//! undone and sees it fail.

mod kernel;
mod plans;
mod starts;
mod walk;

use bootslot::{
    InR, Nv, Reason, Slot, TRIES, commit, r_boots, restart_into, rollback, stage_begin, stage_end,
    up,
};
use starts::starts;
use std::time::{Duration, Instant};
use walk::{Walk, budget};

/// Boots in each sequence.
const DEPTH: u8 = 10;

#[test]
fn every_sequence_of_ten_boots_keeps_the_floors_and_the_rule() {
    let started = Instant::now();
    let starts = starts();
    let mut walk = Walk::new(kernel::REAL);
    walk.run(&starts, DEPTH);
    let elapsed = started.elapsed();
    println!(
        "bootslot model: {} starting states, {} states walked, {} transitions, up to {DEPTH} boots, in {elapsed:?}",
        starts.len(),
        walk.parent.len(),
        walk.transitions,
    );
    // Pinned, so a change that walks less (or more) is seen.
    assert_eq!(
        (starts.len(), walk.parent.len(), walk.transitions),
        (STARTS, STATES, TRANSITIONS)
    );
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
}

/// The walk's size, from its own run.
const STARTS: usize = 4_883;
const STATES: usize = 425_904;
const TRANSITIONS: u64 = 2_795_031;

/// [`budget`]'s bound is reached: a trial of 3 tries that comes up and is
/// reset, then dies before *up*, three times over, boots 5 times before R
/// retires it, because each *up* leaves `came_up` for the boot after.
#[test]
fn a_trial_reset_after_up_then_lost_before_it_boots_twice_per_try_less_one() {
    let mut nv = Nv::default();
    stage_end(&mut nv, Slot::A, 1);
    commit(&mut nv, Slot::A, 1);
    stage_end(&mut nv, Slot::B, 2);
    restart_into(&mut nv, Slot::B);
    let mut trial_boots = 0;
    let mut came_up_last = false;
    loop {
        let slot = nv.one_shot.take().unwrap_or(Slot::R);
        match slot {
            Slot::B => {
                trial_boots += 1;
                if came_up_last {
                    // Dies before *up*: nothing written.
                    came_up_last = false;
                } else {
                    // Comes up, then a reset.
                    up(&mut nv, Slot::B);
                    came_up_last = true;
                }
            }
            Slot::R => match r_boots(&mut nv) {
                InR::Arm(Slot::A) => break,
                InR::Arm(next) => assert_eq!(next, Slot::B),
                InR::Stay => panic!("R stayed with A committed"),
            },
            Slot::A => unreachable!(),
        }
    }
    assert_eq!(trial_boots, budget(TRIES, false));
    assert_eq!(nv.record.slot(Slot::B).reason, Reason::Unbootable);
}

/// The review's case: B's trial comes up (leaving `came_up`), rolls back to
/// A, and A stages B again. The new trial starts with no mark, so it gets
/// `2 * TRIES - 1` boots, not one more.
#[test]
fn a_mark_left_before_a_rollback_never_reaches_the_next_trial() {
    let mut nv = Nv::default();
    stage_end(&mut nv, Slot::A, 1);
    commit(&mut nv, Slot::A, 1);
    stage_end(&mut nv, Slot::B, 2);
    restart_into(&mut nv, Slot::B);
    up(&mut nv, Slot::B);
    assert!(nv.record.came_up);
    assert!(rollback(&mut nv, Slot::B));
    assert!(!nv.record.came_up);
    up(&mut nv, Slot::A);
    stage_begin(&mut nv, Slot::B);
    stage_end(&mut nv, Slot::B, 3);
    restart_into(&mut nv, Slot::B);
    assert!(!nv.record.came_up);
}
