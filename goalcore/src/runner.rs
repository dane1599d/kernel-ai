//! The `goals` agent's loop (row G1 of `docs/plans/agents-os.md`), with no OS in
//! it: each turn it runs a step the person said yes to, or asks the planner for an
//! active goal's next move, proposes a step to the goal (which asks the door for
//! its tier), runs a step that may run and records what came of it, `actlog` entry
//! included; then it posts the check-ins that are due. The OS half
//! (`user/src/bin/goals`) supplies three parts behind traits: the planner (llm's
//! agent `act` jobs), the door (the registry's tiers), and the steps (the shell's
//! door for an agent's call, under `agent=goals`).
//!
//! One step at a time per goal and at most one step a turn in all, so a goal that
//! has work never starves the panel's check-ins.

use crate::{CheckIn, Door, Goal, Goals, Pause, Presence, State, Step, StepDone, Verdict};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// What the planner says a goal does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan<C> {
    /// One step through the door.
    Step(Step<C>),
    /// A question for the person; the goal waits for the answer.
    Clarify(String),
    /// The planner says the goal is met.
    Done(String),
    /// The planner gives up, with why.
    Fail(String),
    /// Nothing yet: ask again next turn (an llm job still running).
    Later,
}

/// Plans a goal's next move. The tokens it returns are what planning spent, charged
/// to the goal.
pub trait Planner<C> {
    fn next(&mut self, goal: &Goal, now: u64) -> (Plan<C>, u64);
}

/// Runs a step the door let run, and says what came of it: the entry the door
/// logged (`agent=goals`), its tokens, whether it failed.
pub trait Steps<C> {
    fn run(&mut self, goal: u64, step: &Step<C>) -> StepDone;
}

/// What a turn did, for the panel and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A step ran: the goal, the step's number, and the entry it made.
    Ran {
        goal: u64,
        step: u32,
        entry: Option<u64>,
    },
    /// The goal waits for the person: "Goal '...' is waiting for you: ...".
    Waiting {
        goal: u64,
        line: String,
    },
    /// The goal is paused by its budget or its deadline.
    Paused {
        goal: u64,
        pause: Pause,
    },
    Done {
        goal: u64,
    },
    Failed {
        goal: u64,
    },
    CheckIn {
        goal: u64,
        check: CheckIn,
    },
}

/// The parts a turn uses: the planner, the door and the steps.
pub struct Parts<'a, C> {
    pub planner: &'a mut dyn Planner<C>,
    pub door: &'a dyn Door<C>,
    pub steps: &'a mut dyn Steps<C>,
}

/// The loop's own state: the steps that asked, kept with the runner's record of
/// their call until the person answers, since a goal keeps only what it shows.
pub struct Runner<C> {
    asked: BTreeMap<u64, Step<C>>,
}

impl<C> Default for Runner<C> {
    fn default() -> Self {
        Self {
            asked: BTreeMap::new(),
        }
    }
}

impl<C: Clone> Runner<C> {
    pub fn new() -> Self {
        Self::default()
    }

    /// One turn over every goal at time `now`, with the person `person`. A step the
    /// person said yes to (`Goal::approve`, on the trusted path) runs first; else
    /// each active goal with no step in flight gets the planner's next move, until
    /// one step runs.
    pub fn turn(
        &mut self,
        goals: &mut Goals,
        now: u64,
        person: Presence,
        parts: &mut Parts<'_, C>,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        if !self.run_answered(goals, parts, &mut events) {
            self.plan_one(goals, now, person, parts, &mut events);
        }
        for (goal, check) in goals.tick(now) {
            events.push(Event::CheckIn { goal, check });
        }
        events
    }

    /// Runs one asked step the person said yes to, and drops those the goal no
    /// longer waits on (a no, an end, a forget): whether one ran.
    fn run_answered(
        &mut self,
        goals: &mut Goals,
        parts: &mut Parts<'_, C>,
        events: &mut Vec<Event>,
    ) -> bool {
        let ids: Vec<u64> = self.asked.keys().copied().collect();
        for id in ids {
            let Some(goal) = goals.get_mut(id) else {
                self.asked.remove(&id);
                continue;
            };
            if goal.state == State::Waiting {
                continue;
            }
            let step = self.asked.remove(&id);
            if let (Some(step), Some(n)) = (step, goal.flight) {
                run(goal, n, &step, parts.steps, events);
                return true;
            }
        }
        false
    }

    fn plan_one(
        &mut self,
        goals: &mut Goals,
        now: u64,
        person: Presence,
        parts: &mut Parts<'_, C>,
        events: &mut Vec<Event>,
    ) {
        let ids: Vec<u64> = goals
            .list()
            .iter()
            .filter(|goal| goal.state == State::Active && goal.flight.is_none())
            .map(|goal| goal.id)
            .collect();
        for id in ids {
            let Some(goal) = goals.get_mut(id) else {
                continue;
            };
            let (plan, tokens) = parts.planner.next(goal, now);
            goal.charge(tokens);
            let ran = match plan {
                Plan::Later => false,
                Plan::Clarify(question) => {
                    if goal.clarify(&question).is_ok() {
                        events.extend(waiting(goal));
                    }
                    false
                }
                Plan::Done(note) => {
                    if goal.complete(now, &note).is_ok() {
                        events.push(Event::Done { goal: id });
                    }
                    false
                }
                Plan::Fail(why) => {
                    if goal.fail(now, &why).is_ok() {
                        events.push(Event::Failed { goal: id });
                    }
                    false
                }
                Plan::Step(step) => self.propose(goal, now, person, step, parts, events),
            };
            if ran {
                return;
            }
        }
    }

    /// Proposes `step` to `goal` and runs it if the door lets it: whether it ran.
    fn propose(
        &mut self,
        goal: &mut Goal,
        now: u64,
        person: Presence,
        step: Step<C>,
        parts: &mut Parts<'_, C>,
        events: &mut Vec<Event>,
    ) -> bool {
        let id = goal.id;
        match goal.propose(now, step.clone(), person, parts.door) {
            Ok(Verdict::Run) => {
                let n = goal.flight.unwrap_or(0);
                run(goal, n, &step, parts.steps, events);
                true
            }
            Ok(Verdict::Ask(_)) => {
                self.asked.insert(id, step);
                events.extend(waiting(goal));
                false
            }
            Ok(Verdict::Over(pause)) => {
                events.push(Event::Paused { goal: id, pause });
                false
            }
            Err(_) => false,
        }
    }
}

/// Runs step `n` of `goal`, in flight, and records what came of it: the entry is
/// kept whatever came of the step (#980's rule).
fn run<C>(
    goal: &mut Goal,
    n: u32,
    step: &Step<C>,
    steps: &mut dyn Steps<C>,
    events: &mut Vec<Event>,
) {
    let done = steps.run(goal.id, step);
    let entry = done.entry;
    let _ = goal.finish_step(done);
    events.push(Event::Ran {
        goal: goal.id,
        step: n,
        entry,
    });
}

fn waiting(goal: &Goal) -> Option<Event> {
    goal.waiting_line().map(|line| Event::Waiting {
        goal: goal.id,
        line,
    })
}

#[cfg(test)]
mod tests;
