//! The model's own check (#1561's review): the walk in `tests/model` runs with
//! each of the kernel's fixes undone in turn, one [`Kernel`] function swapped
//! for the code before the fix, and must fail on every one. A fix the walk
//! cannot tell from its absence is one it does not check.

#![allow(dead_code)]

#[path = "model/kernel.rs"]
mod kernel;
#[path = "model/plans.rs"]
mod plans;
#[path = "model/starts.rs"]
mod starts;
#[path = "model/walk.rs"]
mod walk;

use bootslot::{ArmFailed, Nv, Refused, UpFailed, commit, may_boot, may_set, one_shot_first};
use kernel::{Kernel, REAL};
use std::panic::{self, AssertUnwindSafe};

/// The kernel's fixes, each undone.
const UNDONE: [(&str, Kernel); 12] = [
    (
        // #1561 F2a: *up* told the fault path its record only after writing
        // it and the one-shot.
        "up tells the fault path after its writes",
        Kernel {
            up_writes: |tell, record, one_shot| {
                let written = record()
                    .map_err(UpFailed::Record)
                    .and_then(|()| one_shot().map_err(UpFailed::OneShot));
                tell();
                written
            },
            ..REAL
        },
    ),
    (
        // #1561 F2b: a lost *up* did not stop a re-arm of the running trial.
        "BOOTREC_SET re-arms a trial whose up was lost",
        Kernel {
            may_set: |now, next, one_shot, running, _| may_set(now, next, one_shot, running, false),
            ..REAL
        },
    ),
    (
        "BOOTREC_SET takes a one-shot may_boot refuses",
        Kernel {
            may_set: |now, next, one_shot, running, lost| {
                let one_shot = one_shot.filter(|&slot| may_boot(next, slot));
                may_set(now, next, one_shot, running, lost)
            },
            ..REAL
        },
    ),
    (
        // #1561's review, MEDIUM 1: a stage's set from before probation's
        // commit dropped it.
        "BOOTREC_SET drops a commit the firmware holds",
        Kernel {
            may_set: |now, next, one_shot, running, lost| match may_set(
                now, next, one_shot, running, lost,
            ) {
                Err(Refused::DropsCommit) => Ok(()),
                other => other,
            },
            ..REAL
        },
    ),
    (
        // #1561's review, LOW: only `updated` checked `may_stage`.
        "BOOTREC_SET stages over the committed slot from a trial",
        Kernel {
            may_set: |now, next, one_shot, running, lost| match may_set(
                now, next, one_shot, running, lost,
            ) {
                Err(Refused::StagesCommitted) => Ok(()),
                other => other,
            },
            ..REAL
        },
    ),
    (
        // #1539's review, MEDIUM 3: BOOTREC_SET stored the record before it
        // told the fault path.
        "BOOTREC_SET tells the fault path after the record",
        Kernel {
            set_writes: |tell, record, one_shot| {
                record()?;
                tell(bootslot::Told::New);
                one_shot()
            },
            ..REAL
        },
    ),
    (
        // #1561's review, LOW: R left the one-shot when the record failed
        // after it, so a store that refused every record booted the trial
        // for ever.
        "R keeps the one-shot when the record fails after it",
        Kernel {
            arm_writes: |armed, next, one_shot, record| {
                if one_shot_first(armed, next) {
                    let stored = one_shot(Some(next));
                    record().map_err(ArmFailed::Record)?;
                    stored.map_err(ArmFailed::OneShot)
                } else {
                    record().map_err(ArmFailed::Record)?;
                    one_shot(Some(next)).map_err(ArmFailed::OneShot)
                }
            },
            ..REAL
        },
    ),
    // The kernel's glue around those steps (`bootslot::glue`, #1567's
    // review): each argument `kernel/src/slots.rs` hands on, undone.
    (
        "BOOTREC_SET checks against no record, not the firmware's",
        Kernel {
            bootrec_set: |may, _read, lost, running, next, one_shot, writes| {
                bootslot::glue::bootrec_set(may, || Ok(None), lost, running, next, one_shot, writes)
            },
            ..REAL
        },
    ),
    (
        "BOOTREC_SET is not told up's record was lost",
        Kernel {
            bootrec_set: |may, read, _lost, running, next, one_shot, writes| {
                bootslot::glue::bootrec_set(may, read, false, running, next, one_shot, writes)
            },
            ..REAL
        },
    ),
    (
        "up never tells the fault path its record (BOOTED)",
        Kernel {
            up_step: |writes, _tell, record, one_shot, lost| {
                bootslot::glue::up_step(writes, &mut || {}, record, one_shot, lost)
            },
            ..REAL
        },
    ),
    (
        "up keeps no lost record (UP_LOST)",
        Kernel {
            up_step: |writes, tell, record, one_shot, _lost| {
                bootslot::glue::up_step(writes, tell, record, one_shot, || {})
            },
            ..REAL
        },
    ),
    (
        "probation reads the record before it takes SETTING",
        Kernel {
            commit_step: |hold, read, slot, epoch, set| {
                let record = read()?;
                hold();
                let mut nv = Nv {
                    record,
                    one_shot: Some(slot),
                };
                commit(&mut nv, slot, epoch);
                Some(set(nv))
            },
            ..REAL
        },
    ),
];

/// Boots each walk goes to: the model's own depth.
const DEPTH: u8 = 10;

#[test]
fn the_walk_fails_with_each_fix_undone() {
    let starts = starts::starts();
    // Each failure is expected: keep its panic message out of the output.
    let hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let caught: Vec<(&str, bool)> = UNDONE
        .iter()
        .map(|(name, k)| {
            let failed = panic::catch_unwind(AssertUnwindSafe(|| {
                walk::Walk::new(*k).run(&starts, DEPTH);
            }))
            .is_err();
            (*name, failed)
        })
        .collect();
    panic::set_hook(hook);
    let missed: Vec<&str> = caught
        .iter()
        .filter(|(_, failed)| !failed)
        .map(|(name, _)| *name)
        .collect();
    assert!(
        missed.is_empty(),
        "the walk passes with these undone: {missed:?}"
    );
}
