//! The starting machines the walk begins from.

use crate::walk::{State, budget, committed, fallback, key};
use bootslot::{Reason, Record, Slot, SlotState, TRIES, may_boot};

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
pub fn starts() -> Vec<State> {
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
