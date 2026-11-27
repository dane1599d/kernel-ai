//! The walk: every machine state reached in up to a given number of boots,
//! breadth first, each boot's writes checked as they land.

use crate::kernel::Kernel;
use crate::plans::{Tag, Tape, Write, plans, r_plans};
use bootslot::{MAX_FAILURES, Nv, Reason, Record, Slot, SlotState, TRIES, fault, may_boot};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// The machine: what the firmware holds, and the walk's own count of each
/// trial's boots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct State {
    pub record: Option<Key>,
    pub one_shot: Option<u8>,
    /// Boots of each system slot while it was a trial, since it was staged.
    pub spent: [u8; 3],
    /// What [`spent`](State::spent) may reach.
    pub budget: [u8; 3],
    /// The boot about to start was armed by an R boot that stored the
    /// one-shot but not its record (power lost between the writes, or the
    /// record's write failed): that spares the try by design
    /// ([`bootslot::one_shot_first`]). Rule 8: only a cut does.
    pub spared: bool,
}

/// A record as far as the rules read it: the boot count, the last fault and
/// the priorities only inform (the firmware picks by the one-shot), so they
/// are left out and states that differ only there are walked once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    slots: [(u8, bool, u8, u8, u32); 3],
    armed: Option<u8>,
    safe_once: bool,
    recovery_once: bool,
    min_release_seq: u32,
    min_epoch: u32,
    came_up: bool,
}

pub fn key(record: &Record) -> Key {
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

pub fn slot_of(byte: u8) -> Slot {
    Slot::ALL[usize::from(byte)]
}

pub fn unkey(key: &Key) -> Record {
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
pub fn budget(tries: u8, came_up: bool) -> u8 {
    (2 * tries).saturating_sub(1) + u8::from(came_up)
}

pub fn committed(record: &Record, slot: Slot) -> bool {
    record.slot(slot).successful
}

/// A committed slot R could fall back to: bootable, not below the floors, with
/// failures to spare, and not the armed slot on its last one (R falls back
/// from that one).
pub fn fallback(record: &Record, slot: Slot) -> bool {
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

pub struct Walk {
    k: Kernel,
    /// Each state reached, and the state and boot it was first reached by.
    pub parent: HashMap<State, Option<(State, Step)>>,
    /// How each slot's boot from each record may end, found once.
    outcomes: HashMap<(u8, Option<Key>), Rc<Vec<Outcome>>>,
    pub transitions: u64,
}

/// Applies `write` to the firmware's variables and checks rules 1 to 4.
pub fn apply(state: &mut State, write: Write, slot: Slot, path: &[Step]) {
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
    pub fn new(k: Kernel) -> Walk {
        Walk {
            k,
            parent: HashMap::new(),
            outcomes: HashMap::new(),
            transitions: 0,
        }
    }

    /// Every sequence of up to `depth` boots from `starts`, breadth first, so
    /// each state is walked once, with the most boots left it is reached
    /// with.
    pub fn run(&mut self, starts: &[State], depth: u8) {
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
        let k = self.k;
        let outcomes = self
            .outcomes
            .entry((slot as u8, state.record))
            .or_insert_with(|| Rc::new(outcomes(&k, slot, record, path)))
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
pub struct Outcome {
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
fn outcomes(k: &Kernel, slot: Slot, record: Option<Record>, path: &mut Vec<Step>) -> Vec<Outcome> {
    let (plans, meant) = match (slot, record) {
        (Slot::R, _) => r_plans(k, record),
        // NoRecord: nothing is written, so R installs next.
        (_, None) => (vec![vec![]], None),
        (_, Some(record)) => (plans(k, slot, record), None),
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
                // Rule 8: R's boot that ran to its end (a reset, or staying
                // up) leaves no one-shot its record did not count.
                assert!(
                    !end.spared || cut < plan.len(),
                    "{path:?}: R's boot ended with the one-shot of {:?} stored and its record not: {plan:?}",
                    end.one_shot
                );
            }
            end_at(end, how(End::Cut));
            // Or a fault the kernel records, from what it was last told, in
            // [`bootslot::fault_writes`]'s order, itself cut after each step.
            if let (true, Some((from, tag))) = (faults, told) {
                let mut nv = Nv {
                    record: from,
                    one_shot: None,
                };
                fault(&mut nv, slot, 1, 0);
                let tape = Tape::default();
                let _ = (k.fault_writes)(
                    &mut || {
                        tape.push(Write::Record(nv.record, tag));
                        Ok(())
                    },
                    &mut || {
                        tape.push(Write::OneShot(None));
                        Ok(())
                    },
                );
                let steps = tape.take();
                let mut f = now;
                for (n, &write) in steps.iter().enumerate() {
                    let end = if n + 1 == steps.len() {
                        End::Fault
                    } else {
                        End::FaultCut
                    };
                    path.push(how(end));
                    apply(&mut f, write, slot, path);
                    path.pop();
                    end_at(f, how(end));
                }
            }
        }
    }
    out
}

/// How a boot in a path ended: after which step of which plan.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Read through Debug, in the checks' messages.
pub struct Step {
    slot: Slot,
    plan: usize,
    cut: usize,
    end: End,
}

#[derive(Clone, Copy, Debug)]
pub enum End {
    /// Power lost or a reset.
    Cut,
    /// A fault recorded, cut after its record.
    FaultCut,
    /// A fault recorded.
    Fault,
}
