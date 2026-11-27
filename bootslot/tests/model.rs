//! `bootslot`'s transitions checked exhaustively (plan P6 G46, ADR 0008's
//! review, part 2): every sequence of up to [`DEPTH`] boots from each starting
//! record, where each boot may lose power, or fault, after any one of its
//! writes. The writes are the kernel's (`kernel/src/slots.rs`), in its order:
//!
//! - a system slot's *up*: the record, then the one-shot (`write`);
//! - a `BOOTREC_SET` (a commit, a stage, a rollback, a recovery request): the
//!   record, which the fault path then writes from, then the one-shot (`set`);
//! - the fault path: the fault on the record it was last told, then the
//!   one-shot deleted (`on_fault`);
//! - R: in [`bootslot::arm_writes`]'s order, or the record alone when it stays.
//!
//! The firmware only takes the one-shot (Limine erases it); the kernel never
//! runs [`bootslot::boot`]. After each write the walk checks:
//!
//! 1. the floors never fall, and rise only in a commit's record, to the
//!    release of the slot that committed;
//! 2. no committed slot is below the floors;
//! 3. the next boot is one [`bootslot::may_boot`] allows: R, a committed slot,
//!    or a trial slot with tries left;
//! 4. a trial slot boots no more often than its tries allow before R or a
//!    committed slot boots instead (see [`budget`]).

use bootslot::{
    InR, Nv, Reason, Record, Slot, SlotState, TRIES, commit, fault, install, may_boot,
    one_shot_first, r_boots, recovery, restart_into, rollback, stage_begin, stage_end, up,
};
use std::collections::HashMap;
use std::time::Instant;

/// Boots in each sequence.
const DEPTH: u8 = 8;

/// One variable write, in the order the kernel makes them.
#[derive(Clone, Copy, Debug)]
enum Write {
    Record(Record, Tag),
    OneShot(Option<Slot>),
}

/// What wrote a record, for the floor check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    /// A commit of this slot, after its probation passed.
    Commit(Slot),
    /// A stage of this slot: its tries start again.
    Stage(Slot),
    /// R with no record: both system slots start as trials.
    Install,
    Other,
}

/// The machine: what the firmware holds, and the walk's own count of each
/// trial's boots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct State {
    record: Option<Key>,
    one_shot: Option<u8>,
    /// Boots of each system slot while it was a trial, since it was staged.
    spent: [u8; 3],
    /// What [`spent`](State::spent) may reach.
    budget: [u8; 3],
    /// The boot about to start was armed by an R boot that lost power between
    /// its writes, with the one-shot stored and the record not: a cut that
    /// spares the try by design ([`one_shot_first`]).
    spared: bool,
}

/// A record as far as the rules read it: the boot count, the last fault and
/// the priorities only inform (the firmware picks by the one-shot), so they
/// are left out and states that differ only there are walked once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    slots: [(u8, bool, u8, u8, u32); 3],
    armed: Option<u8>,
    safe_once: bool,
    recovery_once: bool,
    min_release_seq: u32,
    min_epoch: u32,
    came_up: bool,
}

fn key(record: &Record) -> Key {
    Key {
        slots: record.slots.map(|s| {
            (
                s.tries,
                s.successful,
                s.reason as u8,
                s.failures,
                s.release_seq,
            )
        }),
        armed: record.armed.map(|s| s as u8),
        safe_once: record.safe_once,
        recovery_once: record.recovery_once,
        min_release_seq: record.min_release_seq,
        min_epoch: record.min_epoch,
        came_up: record.came_up,
    }
}

fn slot_of(byte: u8) -> Slot {
    Slot::ALL[usize::from(byte)]
}

fn unkey(key: &Key) -> Record {
    let mut record = Record::default();
    for (slot, &(tries, successful, reason, failures, release_seq)) in
        record.slots.iter_mut().zip(&key.slots)
    {
        *slot = SlotState {
            priority: 0,
            tries,
            successful,
            reason: [Reason::None, Reason::Updating, Reason::Unbootable][usize::from(reason)],
            failures,
            release_seq,
        };
    }
    record.armed = key.armed.map(slot_of);
    record.safe_once = key.safe_once;
    record.recovery_once = key.recovery_once;
    record.min_release_seq = key.min_release_seq;
    record.min_epoch = key.min_epoch;
    record.came_up = key.came_up;
    record
}

/// How many boots a trial of `tries` gets before R stops arming it: one try
/// each, except that a boot which dies before *up* after an earlier boot of
/// the same slot came up and was reset (not faulted) spends none, because the
/// kernel never clears `came_up` when a slot boots (lib.rs, `Record::came_up`:
/// "a mark an earlier boot left can spare one more try in R, once"). So at
/// most one spared boot per try after the first, and one for a mark the
/// starting record holds.
fn budget(tries: u8, came_up: bool) -> u8 {
    (2 * tries).saturating_sub(1) + u8::from(came_up)
}

/// What one boot of `slot` may write: each list is the writes of one way the
/// boot goes, beside the record the fault path writes from after each write.
/// The walk then cuts each list after every write, and faults after every
/// write.
fn plans(slot: Slot, record: Option<Record>) -> (Vec<Vec<(Write, Record)>>, Record) {
    let set = |plan: &mut Vec<(Write, Record)>, nv: &Nv, tag: Tag| {
        // BOOTREC_SET: the fault path writes from the record once it is stored.
        plan.push((Write::Record(nv.record, tag), nv.record));
        plan.push((Write::OneShot(nv.one_shot), nv.record));
    };
    let Some(record) = record else {
        if slot != Slot::R {
            // NoRecord: nothing is written, so R installs next.
            return (vec![vec![]], Record::default());
        }
        let mut nv = Nv::default();
        install(&mut nv, 1);
        // write_arm with nothing armed before: the record first.
        let plan = vec![
            (Write::Record(nv.record, Tag::Install), nv.record),
            (Write::OneShot(nv.one_shot), nv.record),
        ];
        return (vec![plan], Record::default());
    };
    let before = Nv {
        record,
        one_shot: None,
    };
    if slot == Slot::R {
        let mut nv = before;
        let mut plan = Vec::new();
        match r_boots(&mut nv) {
            InR::Arm(next) => {
                let rec = (Write::Record(nv.record, Tag::Other), nv.record);
                let shot = (Write::OneShot(Some(next)), nv.record);
                if one_shot_first(record.armed, next) {
                    plan.push(shot);
                    if nv.record != record {
                        plan.push(rec);
                    }
                } else {
                    if nv.record != record {
                        plan.push(rec);
                    }
                    plan.push(shot);
                }
            }
            InR::Stay => {
                if nv.record != record {
                    plan.push((Write::Record(nv.record, Tag::Other), nv.record));
                }
            }
        }
        return (vec![plan], record);
    }
    // A system slot: *up*'s write, then what the boot does.
    let mut upped = before;
    up(&mut upped, slot);
    let mut base = Vec::new();
    if upped.record != record {
        // BOOTED is still the record before *up* until both writes are done.
        base.push((Write::Record(upped.record, Tag::Other), record));
    }
    if upped.one_shot.is_some() {
        base.push((Write::OneShot(upped.one_shot), record));
    }
    if let Some(last) = base.last_mut() {
        last.1 = upped.record;
    }
    let mut out = vec![base.clone()];
    let state = upped.record.slot(slot);
    if !state.successful {
        // Probation passed: the commit reads the record and arms itself.
        let mut nv = Nv {
            record: upped.record,
            one_shot: Some(slot),
        };
        let epoch = nv.record.slot(slot).release_seq;
        commit(&mut nv, slot, epoch);
        let mut plan = base.clone();
        set(&mut plan, &nv, Tag::Commit(slot));
        out.push(plan);
    } else if let Some(other) = slot.other() {
        // `updated` stages the other slot: Updating, written, then a trial
        // of the next release and the restart into it.
        let mut nv = upped;
        stage_begin(&mut nv, other);
        let mut plan = base.clone();
        set(&mut plan, &nv, Tag::Other);
        stage_end(&mut nv, other, state.release_seq + 1);
        restart_into(&mut nv, other);
        set(&mut plan, &nv, Tag::Stage(other));
        out.push(plan);
    }
    let mut nv = upped;
    if rollback(&mut nv, slot) {
        let mut plan = base.clone();
        set(&mut plan, &nv, Tag::Other);
        out.push(plan);
    }
    let mut nv = upped;
    recovery(&mut nv);
    let mut plan = base;
    set(&mut plan, &nv, Tag::Other);
    out.push(plan);
    (out, record)
}

struct Walk {
    /// Each state seen, with the most boots left it was walked with.
    seen: HashMap<State, u8>,
    transitions: u64,
}

/// Applies `write` to the firmware's variables and checks rules 1 and 2.
fn apply(state: &mut State, write: Write, slot: Slot) {
    match write {
        Write::OneShot(one_shot) => state.one_shot = one_shot.map(|s| s as u8),
        Write::Record(record, tag) => {
            if let Some(old) = state.record.as_ref().map(unkey) {
                let raised = record.min_release_seq != old.min_release_seq
                    || record.min_epoch != old.min_epoch;
                assert!(
                    record.min_release_seq >= old.min_release_seq
                        && record.min_epoch >= old.min_epoch,
                    "a floor fell in {slot:?}: {old:?} to {record:?}"
                );
                if raised {
                    let Tag::Commit(by) = tag else {
                        panic!("a floor rose with no commit in {slot:?}: {old:?} to {record:?}");
                    };
                    let committed = record.slot(by);
                    assert!(by == slot && committed.successful, "{record:?}");
                    assert_eq!(committed.release_seq, record.min_release_seq);
                }
            }
            for s in [Slot::A, Slot::B] {
                let state = record.slot(s);
                assert!(
                    !state.successful || (state.reason == Reason::None && !record.below_floor(s)),
                    "committed {s:?} below the floors or retired: {record:?}"
                );
            }
            match tag {
                Tag::Stage(s) => {
                    state.spent[s as usize] = 0;
                    state.budget[s as usize] = budget(TRIES, false);
                }
                Tag::Install => {
                    state.spent = [0; 3];
                    state.budget = [0, budget(TRIES, false), budget(TRIES, false)];
                }
                _ => {}
            }
            state.record = Some(key(&record));
        }
    }
}

impl Walk {
    fn walk(&mut self, state: State, left: u8, path: &mut Vec<String>) {
        let slot = state.one_shot.map_or(Slot::R, slot_of);
        let record = state.record.as_ref().map(unkey);
        if let Some(record) = &record {
            assert!(
                may_boot(record, slot),
                "{path:?}: the next boot is {slot:?} with {record:?}"
            );
        }
        if left == 0 {
            return;
        }
        if self.seen.get(&state).is_some_and(|&had| had >= left) {
            return;
        }
        self.seen.insert(state, left);
        // The firmware takes the one-shot; a trial's boot is counted.
        let mut booted = state;
        booted.one_shot = None;
        booted.spared = false;
        if let Some(record) = &record
            && slot != Slot::R
            && !record.slot(slot).successful
            && !state.spared
        {
            let i = slot as usize;
            booted.spent[i] += 1;
            assert!(
                booted.spent[i] <= booted.budget[i],
                "{path:?}: {slot:?} booted {} times as a trial, its budget {}",
                booted.spent[i],
                booted.budget[i]
            );
        }
        let (plans, first) = plans(slot, record);
        for (p, plan) in plans.iter().enumerate() {
            let mut now = booted;
            let mut faulted_from = first;
            for cut in 0..=plan.len() {
                if cut > 0 {
                    let (write, from) = plan[cut - 1];
                    apply(&mut now, write, slot);
                    faulted_from = from;
                }
                // The boot ends here: power lost, a reset, or a fault the
                // firmware must not record.
                let mut end = now;
                if slot == Slot::R && cut == 1 && plan.len() == 2 {
                    end.spared = matches!(plan[0].0, Write::OneShot(Some(_)));
                }
                self.step(end, left, path, format!("{slot:?} plan {p} cut {cut}"));
                // Or a fault the kernel records, from what it was last told,
                // itself cut after its record.
                if slot != Slot::R && record.is_some() {
                    let mut nv = Nv {
                        record: faulted_from,
                        one_shot: None,
                    };
                    fault(&mut nv, slot, 1, 0);
                    let mut f = now;
                    apply(&mut f, Write::Record(nv.record, Tag::Other), slot);
                    self.step(f, left, path, format!("{slot:?} plan {p} fault {cut} cut"));
                    apply(&mut f, Write::OneShot(None), slot);
                    self.step(f, left, path, format!("{slot:?} plan {p} fault {cut}"));
                }
            }
        }
    }

    fn step(&mut self, next: State, left: u8, path: &mut Vec<String>, how: String) {
        self.transitions += 1;
        path.push(how);
        self.walk(next, left - 1, path);
        path.pop();
    }
}

/// A system slot's starting states: trials with 1 to 3 tries, committed with 0
/// to 2 failures, being written and spent, each of release 1 or 2.
fn slot_states() -> Vec<SlotState> {
    let mut out = Vec::new();
    for release_seq in [1, 2] {
        let base = SlotState {
            release_seq,
            ..SlotState::default()
        };
        for tries in 1..=TRIES {
            out.push(SlotState { tries, ..base });
        }
        for failures in 0..3 {
            out.push(SlotState {
                successful: true,
                failures,
                ..base
            });
        }
        out.push(SlotState {
            reason: Reason::Updating,
            ..base
        });
        out.push(SlotState {
            reason: Reason::Unbootable,
            ..base
        });
    }
    out
}

/// Every starting machine: each pair of slot states, floor, armed slot,
/// `came_up`, recovery request and one-shot, where no committed slot is
/// below the floor and the one-shot names a slot the rule allows; and no
/// record at all, with each one-shot.
fn starts() -> Vec<State> {
    let mut out = Vec::new();
    let slots = slot_states();
    for &a in &slots {
        for &b in &slots {
            for floor in [1, 2] {
                for armed in [None, Some(Slot::A), Some(Slot::B)] {
                    for came_up in [false, true] {
                        for recovery_once in [false, true] {
                            let record = Record {
                                slots: [SlotState::default(), a, b],
                                armed,
                                recovery_once,
                                min_release_seq: floor,
                                min_epoch: floor,
                                came_up,
                                ..Record::default()
                            };
                            let committed_ok = [Slot::A, Slot::B]
                                .into_iter()
                                .all(|s| !record.slot(s).successful || !record.below_floor(s));
                            if !committed_ok {
                                continue;
                            }
                            for one_shot in [None, Some(Slot::A), Some(Slot::B)] {
                                if one_shot.is_some_and(|s| !may_boot(&record, s)) {
                                    continue;
                                }
                                let budget_of = |s: Slot| {
                                    budget(record.slot(s).tries, came_up && armed == Some(s))
                                };
                                out.push(State {
                                    record: Some(key(&record)),
                                    one_shot: one_shot.map(|s| s as u8),
                                    spent: [0; 3],
                                    budget: [0, budget_of(Slot::A), budget_of(Slot::B)],
                                    spared: false,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    for one_shot in [None, Some(Slot::A), Some(Slot::B)] {
        out.push(State {
            record: None,
            one_shot: one_shot.map(|s| s as u8),
            spent: [0; 3],
            budget: [0; 3],
            spared: false,
        });
    }
    out
}

#[test]
fn every_sequence_of_eight_boots_keeps_the_floors_and_the_rule() {
    let started = Instant::now();
    let starts = starts();
    let mut walk = Walk {
        seen: HashMap::new(),
        transitions: 0,
    };
    for &start in &starts {
        walk.walk(start, DEPTH, &mut Vec::new());
    }
    println!(
        "bootslot model: {} starting states, {} states walked, {} transitions, up to {DEPTH} boots, in {:?}",
        starts.len(),
        walk.seen.len(),
        walk.transitions,
        started.elapsed()
    );
    assert!(starts.len() > 10_000);
}

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
