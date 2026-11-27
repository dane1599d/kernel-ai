//! What one boot may write: the steps of each way it can go, in the kernel's
//! order, recorded by driving [`Kernel`]'s functions with closures that note
//! each step on a [`Tape`].

use crate::kernel::Kernel;
use crate::walk::{committed, fallback};
use bootslot::glue::SetFailed;
use bootslot::{
    InR, Nv, Record, Refused, Slot, Told, install, r_boots, recovery, restart_into, rollback,
    stage_begin, stage_end, up,
};
use std::cell::{Cell, RefCell};

/// One step of a boot, in the order the kernel takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// The record stored in the firmware.
    Record(Record, Tag),
    /// The one-shot stored, or deleted.
    OneShot(Option<Slot>),
    /// What the fault path writes from from now on (`BOOTED`): no firmware
    /// write.
    Told(Record, Tag),
}

/// What wrote a record, for the checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    /// *Up* of this slot.
    Up(Slot),
    /// A commit of this slot, after its probation passed.
    Commit(Slot),
    /// A stage beginning over this slot: it is being written.
    StageBegin(Slot),
    /// A stage of this slot: its tries start again.
    Stage(Slot),
    /// R with no record: both system slots start as trials.
    Install,
    Other,
}

/// A recording of the steps one boot takes.
#[derive(Default)]
pub struct Tape {
    steps: RefCell<Vec<Write>>,
}

impl Tape {
    pub fn push(&self, write: Write) {
        self.steps.borrow_mut().push(write);
    }

    pub fn take(self) -> Vec<Write> {
        self.steps.into_inner()
    }
}

/// A system slot's boot once it is up, as its plan goes: the record the
/// firmware holds (BOOTREC_GET reads it, and BOOTREC_SET compares with it),
/// what the fault path was last told, and whether *up*'s record was lost.
#[derive(Clone, Copy)]
struct Up {
    k: Kernel,
    slot: Slot,
    fw: Record,
    told: (Record, Tag),
    lost: bool,
}

impl Up {
    /// The kernel's BOOTREC_SET of `next` (`slots::set`): refused as
    /// [`bootslot::may_set`] says before anything is written, else written in
    /// [`bootslot::set_writes`]'s order. The record's write fails when
    /// `fails`; the one-shot's failing is the same as a cut after the record.
    /// Whether the record was stored.
    fn bootrec_set(
        &mut self,
        tape: &Tape,
        next: Nv,
        tag: Tag,
        fails: bool,
    ) -> Result<bool, Refused> {
        let old = self.told;
        let told = Cell::new(old);
        let stored = Cell::new(false);
        let fw = self.fw;
        let set_writes = self.k.set_writes;
        let set = (self.k.bootrec_set)(
            self.k.may_set,
            &mut || Ok(Some(fw)),
            self.lost,
            Some(self.slot),
            &next.record,
            next.one_shot,
            &mut || {
                set_writes(
                    &mut |t| {
                        let now = match t {
                            Told::New => (next.record, tag),
                            Told::Old => old,
                        };
                        told.set(now);
                        tape.push(Write::Told(now.0, now.1));
                    },
                    &mut || {
                        if fails {
                            return Err(());
                        }
                        tape.push(Write::Record(next.record, tag));
                        stored.set(true);
                        Ok(())
                    },
                    &mut || {
                        tape.push(Write::OneShot(next.one_shot));
                        Ok(())
                    },
                )
            },
        );
        if let Err(SetFailed::Refused(refused)) = set {
            return Err(refused);
        }
        self.told = told.get();
        if stored.get() {
            self.fw = next.record;
        }
        Ok(stored.get())
    }

    /// `updated`'s `set`: BOOTREC_SET, and after a Busy the record rebased
    /// onto the commit the firmware holds and set once more.
    fn user_set(&mut self, tape: &Tape, mut next: Nv, tag: Tag, fails: bool) -> bool {
        match self.bootrec_set(tape, next, tag, fails) {
            Ok(stored) => stored,
            Err(Refused::DropsCommit) if (self.k.rebase)(&mut next.record, &self.fw, self.slot) => {
                self.bootrec_set(tape, next, tag, fails) == Ok(true)
            }
            Err(_) => false,
        }
    }

    /// Probation's commit, from the record the firmware holds now, read and
    /// set under SETTING (`slots::commit`).
    fn commit(&mut self, tape: &Tape) {
        self.commit_racing(tape, None);
    }

    /// [`Up::commit`], with `updated`'s BOOTREC_SET of `racing` (from the
    /// record the firmware holds when it runs) trying to land just after the
    /// commit reads the record: while SETTING is held it waits until the
    /// commit's set is done. `racing` gives the set's tag, or `None` when
    /// `updated` would not set at all.
    fn commit_racing(&mut self, tape: &Tape, racing: Option<Racer>) {
        if committed(&self.fw, self.slot) {
            return;
        }
        let epoch = self.fw.slot(self.slot).release_seq;
        let me = RefCell::new(*self);
        let held = Cell::new(false);
        let waiting = Cell::new(racing);
        let race = |up: &mut Up| {
            if let Some(change) = waiting.take() {
                let mut nv = Nv {
                    record: up.fw,
                    one_shot: None,
                };
                if let Some(tag) = change(&mut nv, up.slot) {
                    up.user_set(tape, nv, tag, false);
                }
            }
        };
        let _ = (self.k.commit_step)(
            &mut || held.set(true),
            &mut || {
                let read = me.borrow().fw;
                if !held.get() {
                    race(&mut me.borrow_mut());
                }
                Some(read)
            },
            self.slot,
            epoch,
            &mut |nv| {
                let mut up = me.borrow_mut();
                let tag = Tag::Commit(up.slot);
                up.bootrec_set(tape, nv, tag, false).map(drop).map_err(drop)
            },
        );
        *self = me.into_inner();
        race(self);
    }
}

/// A change `updated` makes to the record it reads, racing probation's
/// commit, and the tag of its set; `None` when it would not set.
type Racer = fn(&mut Nv, Slot) -> Option<Tag>;

/// R's boot: what it may write, and the record it means to store if that
/// changed. Checks rule 7.
pub fn r_plans(k: &Kernel, record: Option<Record>) -> (Vec<Vec<Write>>, Option<Record>) {
    let before = Nv {
        record: record.unwrap_or_default(),
        one_shot: None,
    };
    let mut nv = before;
    let decision = match record {
        Some(_) => r_boots(&mut nv),
        None => {
            install(&mut nv, 1);
            InR::Arm(Slot::A)
        }
    };
    let tag = if record.is_some() {
        Tag::Other
    } else {
        Tag::Install
    };
    if let (Some(record), InR::Stay) = (record, decision) {
        assert!(
            record.recovery_once || ![Slot::A, Slot::B].into_iter().any(|s| fallback(&record, s)),
            "R stays up with a committed slot to fall back to: {record:?}"
        );
    }
    let changed = record.is_none() || nv.record != before.record;
    let mut out = Vec::new();
    // The one-shot's write failing (0) or the record's (1), or neither.
    for fail in [None, Some(0), Some(1)] {
        let tape = Tape::default();
        match decision {
            InR::Arm(next) => {
                let _ = (k.arm_writes)(
                    before.record.armed,
                    next,
                    &mut |one_shot| {
                        if one_shot.is_some() && fail == Some(0) {
                            return Err(());
                        }
                        tape.push(Write::OneShot(one_shot));
                        Ok(())
                    },
                    &mut || {
                        if !changed {
                            return Ok(());
                        }
                        if fail == Some(1) {
                            return Err(());
                        }
                        tape.push(Write::Record(nv.record, tag));
                        Ok(())
                    },
                );
            }
            InR::Stay => {
                if changed && fail != Some(1) {
                    tape.push(Write::Record(nv.record, tag));
                }
            }
        }
        let plan = tape.take();
        if !out.contains(&plan) {
            out.push(plan);
        }
    }
    (out, changed.then_some(nv.record))
}

/// What a system slot's boot may do once it is up: nothing more; a commit if
/// it is a trial; a rollback; a recovery request; a stage of the other slot
/// (`updated`'s, by the update disk, the network or `step=check`), which the
/// kernel refuses over the committed slot from a trial. While it is a trial,
/// probation may commit it between any read `updated` made and each of its
/// BOOTREC_SETs, or after them. A failed BOOTREC_SET ends what the boot does.
fn tails(up: Up) -> Vec<Vec<Write>> {
    let trial = !committed(&up.fw, up.slot);
    let mut out = vec![vec![]];
    let mut run = |go: &dyn Fn(&mut Up, &Tape)| {
        let mut up = up;
        let tape = Tape::default();
        go(&mut up, &tape);
        let plan = tape.take();
        if !out.contains(&plan) {
            out.push(plan);
        }
    };
    if trial {
        run(&|up, tape| up.commit(tape));
        // `updated`'s set racing probation's commit for SETTING (#1567's
        // review): a recovery request, a rollback (none when `rollback`
        // finds nothing to go back to), and a stage's beginning, the one
        // that leaves this slot's state as the commit read it, so rule 9
        // sees a commit the race undid (#1598's review).
        let racers: [Racer; 3] = [
            |nv, _| {
                recovery(nv);
                Some(Tag::Other)
            },
            |nv, slot| rollback(nv, slot).then_some(Tag::Other),
            |nv, slot| {
                let other = slot.other()?;
                stage_begin(nv, other);
                Some(Tag::StageBegin(other))
            },
        ];
        for racer in racers {
            run(&|up, tape| up.commit_racing(tape, Some(racer)));
        }
    }
    let commits: &[bool] = if trial { &[false, true] } else { &[false] };
    for &commits in commits {
        run(&|up, tape| {
            let mut nv = Nv {
                record: up.fw,
                one_shot: None,
            };
            if rollback(&mut nv, up.slot) {
                if commits {
                    up.commit(tape);
                }
                up.user_set(tape, nv, Tag::Other, false);
            }
        });
        run(&|up, tape| {
            let mut nv = Nv {
                record: up.fw,
                one_shot: None,
            };
            recovery(&mut nv);
            if commits {
                up.commit(tape);
            }
            up.user_set(tape, nv, Tag::Other, false);
        });
    }
    let Some(other) = up.slot.other() else {
        return out;
    };
    // The stage, from the record `updated` read at its start: Updating,
    // written, then a trial of a release above every one it has run, and the
    // restart into it. Probation's commit lands at `at`: never (0), before
    // the first set (1), between the two (2) or after them (3). Only the first
    // set is walked failing: a failed one ends where a cut just before it does.
    let ats = if trial { 0..4 } else { 0..1 };
    for at in ats {
        for first_fails in [true, false] {
            run(&|up, tape| {
                let read = up.fw;
                let release = read.slot(up.slot).release_seq.max(read.min_release_seq) + 1;
                let mut nv = Nv {
                    record: read,
                    one_shot: Some(up.slot),
                };
                stage_begin(&mut nv, other);
                if at == 1 {
                    up.commit(tape);
                }
                if !up.user_set(tape, nv, Tag::StageBegin(other), first_fails) {
                    return;
                }
                if at == 2 {
                    up.commit(tape);
                }
                // From `updated`'s own copy, not read again.
                stage_end(&mut nv, other, release);
                restart_into(&mut nv, other);
                if !up.user_set(tape, nv, Tag::Stage(other), false) {
                    return;
                }
                if at == 3 {
                    up.commit(tape);
                }
            });
        }
    }
    out
}

/// What one boot of `slot` may write: each list is the steps of one way the
/// boot goes. The walk then cuts each list after every step, and faults after
/// every step.
pub fn plans(k: &Kernel, slot: Slot, record: Record) -> Vec<Vec<Write>> {
    let mut upped = Nv {
        record,
        one_shot: None,
    };
    up(&mut upped, slot);
    let tag = Tag::Up(slot);
    let changed = upped.record != record;
    let mut out = Vec::new();
    // *Up*'s writes: both, the record's failing (which skips the one-shot),
    // or the one-shot's.
    for fail in [None, Some(0), Some(1)] {
        let tape = Tape::default();
        let told = Cell::new((record, Tag::Other));
        let stored = Cell::new(!changed);
        let lost = Cell::new(false);
        let written = (k.up_step)(
            k.up_writes,
            &mut || {
                told.set((upped.record, tag));
                tape.push(Write::Told(upped.record, tag));
            },
            &mut || {
                if !changed {
                    return Ok(());
                }
                if fail == Some(0) {
                    return Err(());
                }
                tape.push(Write::Record(upped.record, tag));
                stored.set(true);
                Ok(())
            },
            &mut || {
                if upped.one_shot.is_none() {
                    return Ok(());
                }
                if fail == Some(1) {
                    return Err(());
                }
                tape.push(Write::OneShot(upped.one_shot));
                Ok(())
            },
            &mut || lost.set(true),
        );
        let _ = written;
        let base = tape.take();
        let fw = if stored.get() { upped.record } else { record };
        let up = Up {
            k: *k,
            slot,
            fw,
            told: told.get(),
            lost: lost.get(),
        };
        for tail in tails(up) {
            let mut plan = base.clone();
            plan.extend(tail);
            if !out.contains(&plan) {
                out.push(plan);
            }
        }
    }
    out
}
