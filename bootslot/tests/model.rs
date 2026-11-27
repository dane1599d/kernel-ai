//! `bootslot`'s transitions checked exhaustively (plan P6 G46, ADR 0008's
//! review, part 2): every sequence of up to [`DEPTH`] boots from each starting
//! record, where each boot may lose power, or fault, after any one of its
//! steps, and a firmware write may fail. The steps are the kernel's
//! (`kernel/src/slots.rs`), in its order:
//!
//! - a system slot's *up*: the fault path told the new record, then the
//!   record, then the one-shot (`write`);
//! - a `BOOTREC_SET` (a commit, a stage, a rollback, a recovery request): in
//!   [`bootslot::set_writes`]'s order, driven here with closures that record
//!   each step: the fault path told the new record, the record, the one-shot;
//! - the fault path: the fault on the record it was last told, then the
//!   one-shot deleted (`on_fault`);
//! - R: in [`bootslot::arm_writes`]'s order, driven the same way, or the
//!   record alone when it stays.
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
//!    committed slot boots instead (see [`budget`]);
//! 7. R stays up only when asked to, or when no committed slot is left that
//!    it could fall back to.

use bootslot::{
    InR, MAX_FAILURES, Nv, Reason, Record, Slot, SlotState, TRIES, Told, arm_writes, commit, fault,
    install, may_boot, may_stage, r_boots, recovery, restart_into, rollback, set_writes,
    stage_begin, stage_end, up,
};
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Boots in each sequence.
const DEPTH: u8 = 10;

/// One step of a boot, in the order the kernel takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Write {
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
enum Tag {
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
    /// The boot about to start was armed by an R boot that stored the
    /// one-shot but not its record (power lost between the writes, or the
    /// record's write failed): that spares the try by design
    /// ([`bootslot::one_shot_first`]).
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
/// each, except that a boot which dies before *up*, after an earlier boot of
/// the same slot came up and was reset (not faulted), spends none, because
/// the kernel never clears `came_up` when a slot boots (`Record::came_up`).
/// So one spared boot per *up* but the last, which spends the last try, and
/// one for a mark `came_up` already holds for the slot: `2 * tries - 1`, or
/// one more with the mark. Rule 4 keeps a mark from reaching a fresh trial,
/// so a stage or an install starts at `2 * TRIES - 1`; the budget still
/// counts the mark the record holds, so the walk is not told it is clear.
fn budget(tries: u8, came_up: bool) -> u8 {
    (2 * tries).saturating_sub(1) + u8::from(came_up)
}

fn committed(record: &Record, slot: Slot) -> bool {
    record.slot(slot).successful
}

/// A committed slot R could fall back to: bootable, not below the floors, with
/// failures to spare, and not the armed slot on its last one (R falls back
/// from that one).
fn fallback(record: &Record, slot: Slot) -> bool {
    let state = record.slot(slot);
    let last = if record.armed == Some(slot) {
        MAX_FAILURES - 1
    } else {
        MAX_FAILURES
    };
    committed(record, slot)
        && state.reason == Reason::None
        && !record.below_floor(slot)
        && state.failures < last
}

/// A recording of the steps one boot takes, for [`set_writes`] and
/// [`arm_writes`] to drive.
#[derive(Default)]
struct Tape {
    steps: RefCell<Vec<Write>>,
}

impl Tape {
    fn push(&self, write: Write) {
        self.steps.borrow_mut().push(write);
    }
}

/// One BOOTREC_SET of `nv` with `tag`, as the kernel takes it, onto `tape`,
/// with `told` what the fault path was told before. The kernel refuses a
/// one-shot [`may_boot`] does not allow before it writes anything
/// (`process/syscall/update.rs`), and in a boot whose *up* was not stored
/// (`lost`), one that arms that slot as a trial (`slots::set`, `UP_LOST`).
/// The record's write may fail (`fails`); the one-shot's failing is the same
/// as a cut after the record. Whether it was set.
fn set(
    tape: &Tape,
    nv: &Nv,
    tag: Tag,
    told: (Record, Tag),
    fails: bool,
    lost: Option<Slot>,
) -> bool {
    if nv.one_shot.is_some_and(|next| !may_boot(&nv.record, next)) {
        return false;
    }
    if let Some(next) = nv.one_shot.filter(|&next| lost == Some(next))
        && !committed(&nv.record, next)
    {
        return false;
    }
    set_writes(
        |t| match t {
            Told::New => tape.push(Write::Told(nv.record, tag)),
            Told::Old => tape.push(Write::Told(told.0, told.1)),
        },
        || {
            if fails {
                return Err(());
            }
            tape.push(Write::Record(nv.record, tag));
            Ok(())
        },
        || {
            tape.push(Write::OneShot(nv.one_shot));
            Ok(())
        },
    )
    .is_ok()
}

/// R's boot: what it may write, and the record it means to store if that
/// changed. Checks rule 7.
fn r_plans(record: Option<Record>) -> (Vec<Vec<Write>>, Option<Record>) {
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
                let _ = arm_writes(
                    before.record.armed,
                    next,
                    || {
                        if fail == Some(0) {
                            return Err(());
                        }
                        tape.push(Write::OneShot(Some(next)));
                        Ok(())
                    },
                    || {
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
        let plan = tape.steps.into_inner();
        if !out.contains(&plan) {
            out.push(plan);
        }
    }
    (out, changed.then_some(nv.record))
}

/// What a system slot's boot may do once it is up, from `fw`, the record the
/// firmware holds (BOOTREC_GET reads it), with the fault path told `told`:
/// nothing more; a commit if it is a trial; a rollback; a recovery request; a
/// stage of the other slot, if [`may_stage`] lets it (`updated`). Each
/// BOOTREC_SET may fail, which ends what the boot does.
fn tails(slot: Slot, fw: Record, told: (Record, Tag), lost: bool) -> Vec<Vec<Write>> {
    let lost = lost.then_some(slot);
    let mut out = vec![vec![]];
    // A BOOTREC_SET whose record fails to write ends where a cut just before
    // it does (the fault path is told the old record again), so only the
    // first stage set is walked failing, to drive that path.
    let mut one = |nv: Nv, tag: Tag| {
        let tape = Tape::default();
        set(&tape, &nv, tag, told, false, lost);
        out.push(tape.steps.into_inner());
    };
    if !committed(&fw, slot) {
        // Probation passed: the commit reads the record and arms itself.
        let mut nv = Nv {
            record: fw,
            one_shot: Some(slot),
        };
        let epoch = nv.record.slot(slot).release_seq;
        commit(&mut nv, slot, epoch);
        one(nv, Tag::Commit(slot));
    }
    let mut nv = Nv {
        record: fw,
        one_shot: None,
    };
    if rollback(&mut nv, slot) {
        one(nv, Tag::Other);
    }
    let mut nv = Nv {
        record: fw,
        one_shot: None,
    };
    recovery(&mut nv);
    one(nv, Tag::Other);
    if let Some(other) = slot.other().filter(|_| may_stage(&fw, slot)) {
        // `updated` stages the other slot: Updating, written, then a trial of
        // a release above every one it has run, and the restart into it.
        let release = fw.slot(slot).release_seq.max(fw.min_release_seq) + 1;
        let mut nv = Nv {
            record: fw,
            one_shot: Some(slot),
        };
        stage_begin(&mut nv, other);
        for (first_fails, second_fails) in [(true, false), (false, false)] {
            let tape = Tape::default();
            if set(&tape, &nv, Tag::StageBegin(other), told, first_fails, lost) {
                let begun = (nv.record, Tag::StageBegin(other));
                let mut staged = nv;
                stage_end(&mut staged, other, release);
                restart_into(&mut staged, other);
                set(&tape, &staged, Tag::Stage(other), begun, second_fails, lost);
            }
            out.push(tape.steps.into_inner());
        }
    }
    out
}

/// What one boot of `slot` may write: each list is the steps of one way the
/// boot goes. The walk then cuts each list after every step, and faults after
/// every step.
fn plans(slot: Slot, record: Record) -> Vec<Vec<Write>> {
    let mut upped = Nv {
        record,
        one_shot: None,
    };
    up(&mut upped, slot);
    let told = (upped.record, Tag::Up(slot));
    let changed = upped.record != record;
    let mut out = Vec::new();
    // *Up*'s write, the fault path told its record first: both, the record's
    // failing (which skips the one-shot), or the one-shot's.
    for fail in [None, Some(0), Some(1)] {
        if fail == Some(0) && !changed || fail == Some(1) && upped.one_shot.is_none() {
            continue;
        }
        let mut base = vec![Write::Told(told.0, told.1)];
        if changed && fail != Some(0) {
            base.push(Write::Record(upped.record, Tag::Up(slot)));
        }
        if upped.one_shot.is_some() && fail.is_none() {
            base.push(Write::OneShot(upped.one_shot));
        }
        let fw = if fail == Some(0) {
            record
        } else {
            upped.record
        };
        for tail in tails(slot, fw, told, fail == Some(0)) {
            let mut plan = base.clone();
            plan.extend(tail);
            if !out.contains(&plan) {
                out.push(plan);
            }
        }
    }
    out
}

struct Walk {
    /// Each state reached, and the state and boot it was first reached by.
    parent: HashMap<State, Option<(State, Step)>>,
    /// How each slot's boot from each record may end, found once.
    outcomes: HashMap<(u8, Option<Key>), Rc<Vec<Outcome>>>,
    transitions: u64,
}

/// Applies `write` to the firmware's variables and checks rules 1 to 4.
fn apply(state: &mut State, write: Write, slot: Slot, path: &[Step]) {
    let (record, tag) = match write {
        Write::OneShot(one_shot) => {
            state.one_shot = one_shot.map(|s| s as u8);
            return;
        }
        Write::Told(..) => return,
        Write::Record(record, tag) => (record, tag),
    };
    if let Some(old) = state.record.as_ref().map(unkey) {
        let raised =
            record.min_release_seq != old.min_release_seq || record.min_epoch != old.min_epoch;
        assert!(
            record.min_release_seq >= old.min_release_seq && record.min_epoch >= old.min_epoch,
            "{path:?}: a floor fell in {slot:?} ({tag:?}): {old:?} to {record:?}"
        );
        if raised {
            let Tag::Commit(by) = tag else {
                panic!(
                    "{path:?}: a floor rose with no commit in {slot:?} ({tag:?}): {old:?} to {record:?}"
                );
            };
            let committed = record.slot(by);
            assert!(by == slot && committed.successful, "{path:?}: {record:?}");
            assert_eq!(committed.release_seq, record.min_release_seq);
        }
        for s in [Slot::A, Slot::B] {
            if !committed(&old, s) {
                continue;
            }
            if committed(&record, s) {
                assert_eq!(
                    record.slot(s).release_seq,
                    old.slot(s).release_seq,
                    "{path:?}: committed {s:?} changed its release in {slot:?} ({tag:?}): {record:?}"
                );
                continue;
            }
            let staged = tag == Tag::StageBegin(s);
            let retired = s.other().is_some_and(|o| tag == Tag::Commit(o))
                && record.slot(s).reason == Reason::Unbootable
                && record.below_floor(s);
            assert!(
                staged || retired,
                "{path:?}: committed {s:?} lost its commit in {slot:?} ({tag:?}): {old:?} to {record:?}"
            );
        }
        let had = [Slot::A, Slot::B].into_iter().any(|s| committed(&old, s));
        assert!(
            !had || [Slot::A, Slot::B]
                .into_iter()
                .any(|s| committed(&record, s)),
            "{path:?}: the last committed slot went in {slot:?} ({tag:?}): {old:?} to {record:?}"
        );
        if record.came_up {
            let by_up = matches!(tag, Tag::Up(s) if record.armed == Some(s));
            let kept = old.came_up && old.armed == record.armed;
            assert!(
                by_up || kept,
                "{path:?}: came_up reached another arm in {slot:?} ({tag:?}): {old:?} to {record:?}"
            );
        }
    }
    for s in [Slot::A, Slot::B] {
        let state = record.slot(s);
        assert!(
            !state.successful || (state.reason == Reason::None && !record.below_floor(s)),
            "{path:?}: committed {s:?} below the floors or retired: {record:?}"
        );
    }
    let budget_of = |s: Slot| budget(TRIES, record.came_up && record.armed == Some(s));
    match tag {
        Tag::Stage(s) => {
            state.spent[s as usize] = 0;
            state.budget[s as usize] = budget_of(s);
        }
        Tag::Install => {
            state.spent = [0; 3];
            state.budget = [0, budget_of(Slot::A), budget_of(Slot::B)];
        }
        _ => {}
    }
    state.record = Some(key(&record));
}

impl Walk {
    /// Every sequence of up to `depth` boots from `starts`, breadth first, so
    /// each state is walked once, with the most boots left it is reached
    /// with.
    fn run(&mut self, starts: &[State], depth: u8) {
        let mut frontier = Vec::new();
        for &start in starts {
            if self.parent.insert(start, None).is_none() {
                frontier.push(start);
            }
        }
        for _ in 0..depth {
            let mut next = Vec::new();
            for state in frontier {
                for (end, how) in self.boot(state) {
                    self.transitions += 1;
                    if let Entry::Vacant(first) = self.parent.entry(end) {
                        first.insert(Some((state, how)));
                        next.push(end);
                    }
                }
            }
            frontier = next;
        }
        for state in frontier {
            self.check_next(state, &self.path(state));
        }
    }

    /// The boots that reached `state` first.
    fn path(&self, mut state: State) -> Vec<Step> {
        let mut path = Vec::new();
        while let Some(&Some((from, how))) = self.parent.get(&state) {
            path.push(how);
            state = from;
        }
        path.reverse();
        path
    }

    /// Rule 5, for the boot `state` starts.
    fn check_next(&self, state: State, path: &[Step]) {
        let slot = state.one_shot.map_or(Slot::R, slot_of);
        if let Some(record) = state.record.as_ref().map(unkey) {
            assert!(
                may_boot(&record, slot),
                "{path:?}: the next boot is {slot:?} with {record:?}"
            );
        }
    }

    /// Each way the boot `state` starts may end, once each.
    fn boot(&mut self, state: State) -> Vec<(State, Step)> {
        let path = &mut self.path(state);
        self.check_next(state, path);
        let slot = state.one_shot.map_or(Slot::R, slot_of);
        let record = state.record.as_ref().map(unkey);
        // The firmware takes the one-shot; a trial's boot is counted.
        let mut booted = state;
        booted.one_shot = None;
        booted.spared = false;
        if let Some(record) = &record
            && slot != Slot::R
            && !committed(record, slot)
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
        let outcomes = self
            .outcomes
            .entry((slot as u8, state.record))
            .or_insert_with(|| Rc::new(outcomes(slot, record, path)))
            .clone();
        let mut next = Vec::new();
        let mut seen = HashSet::new();
        for o in outcomes.iter() {
            let mut end = booted;
            end.record = o.record;
            end.one_shot = o.one_shot;
            end.spared = o.spared;
            for (i, reset) in o.resets.iter().enumerate() {
                if let Some(budget) = *reset {
                    end.spent[i] = 0;
                    end.budget[i] = budget;
                }
            }
            if seen.insert(end) {
                next.push((end, o.how));
            }
        }
        next
    }
}

/// One way a boot from a record ends: what the firmware then holds, and the
/// trials it staged (their budgets); the rest of the state carries over.
struct Outcome {
    record: Option<Key>,
    one_shot: Option<u8>,
    spared: bool,
    resets: [Option<u8>; 3],
    how: Step,
}

/// A spent count no trial reaches, to see which a boot reset.
const UNSET: u8 = u8::MAX;

/// Each way a boot of `slot` from `record` ends, once each, after checking
/// every write on the way (rules 1 to 4 and 7). What the firmware holds
/// after a boot depends only on these two, since the boot starts with the
/// one-shot taken: so each pair is walked once, and each state that reaches
/// it only carries its trials' counts over.
fn outcomes(slot: Slot, record: Option<Record>, path: &mut Vec<Step>) -> Vec<Outcome> {
    let (plans, meant) = match (slot, record) {
        (Slot::R, _) => r_plans(record),
        // NoRecord: nothing is written, so R installs next.
        (_, None) => (vec![vec![]], None),
        (_, Some(record)) => (plans(slot, record), None),
    };
    let faults = slot != Slot::R && record.is_some();
    let start = State {
        record: record.as_ref().map(key),
        one_shot: None,
        spent: [UNSET; 3],
        budget: [0; 3],
        spared: false,
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut end_at = |end: State, how: Step| {
        if seen.insert(end) {
            out.push(Outcome {
                record: end.record,
                one_shot: end.one_shot,
                spared: end.spared,
                resets: [0, 1, 2].map(|i| (end.spent[i] != UNSET).then_some(end.budget[i])),
                how,
            });
        }
    };
    for (p, plan) in plans.iter().enumerate() {
        let mut now = start;
        let mut told = record.map(|r| (r, Tag::Other));
        for cut in 0..=plan.len() {
            let how = |end| Step {
                slot,
                plan: p,
                cut,
                end,
            };
            if cut > 0 {
                let write = plan[cut - 1];
                if let Write::Told(r, tag) = write {
                    told = Some((r, tag));
                }
                path.push(how(End::Cut));
                apply(&mut now, write, slot, path);
                path.pop();
            }
            // The boot ends here: power lost, a reset, or a fault the
            // firmware must not record.
            let mut end = now;
            if let Some(meant) = &meant {
                end.spared = end.one_shot.is_some() && end.record != Some(key(meant));
            }
            end_at(end, how(End::Cut));
            // Or a fault the kernel records, from what it was last told,
            // itself cut after its record.
            if let (true, Some((from, tag))) = (faults, told) {
                let mut nv = Nv {
                    record: from,
                    one_shot: None,
                };
                fault(&mut nv, slot, 1, 0);
                let mut f = now;
                path.push(how(End::FaultCut));
                apply(&mut f, Write::Record(nv.record, tag), slot, path);
                path.pop();
                end_at(f, how(End::FaultCut));
                apply(&mut f, Write::OneShot(None), slot, path);
                end_at(f, how(End::Fault));
            }
        }
    }
    out
}

/// How a boot in a path ended: after which step of which plan.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Read through Debug, in the checks' messages.
struct Step {
    slot: Slot,
    plan: usize,
    cut: usize,
    end: End,
}

#[derive(Clone, Copy, Debug)]
enum End {
    /// Power lost or a reset.
    Cut,
    /// A fault recorded, cut after its record.
    FaultCut,
    /// A fault recorded.
    Fault,
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

/// Every starting machine that keeps the rules: each pair of slot states,
/// floor, armed slot, `came_up`, recovery request and one-shot, with no
/// committed slot below the floor; `came_up` only for an armed trial; nothing
/// armed only with no committed slot to fall back to (only a stay disarms R,
/// and then none is left); and the one-shot, if any, the armed slot, which
/// the rule allows (every write that stores a one-shot arms the same slot in
/// the record). And no record at all, with each one-shot.
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
                                .all(|s| !committed(&record, s) || !record.below_floor(s));
                            let came_up_ok =
                                !came_up || armed.is_some_and(|s| !committed(&record, s));
                            let armed_ok = armed.is_some()
                                || ![Slot::A, Slot::B].into_iter().any(|s| fallback(&record, s));
                            if !(committed_ok && came_up_ok && armed_ok) {
                                continue;
                            }
                            for one_shot in [None, armed] {
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
                                if armed.is_none() {
                                    break;
                                }
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
fn every_sequence_of_ten_boots_keeps_the_floors_and_the_rule() {
    let started = Instant::now();
    let starts = starts();
    let mut walk = Walk {
        parent: HashMap::new(),
        outcomes: HashMap::new(),
        transitions: 0,
    };
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
const STATES: usize = 391_156;
const TRANSITIONS: u64 = 2_257_183;

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
