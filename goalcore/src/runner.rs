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

use crate::{
    CheckIn, Door, Goal, Goals, Pause, Presence, Refusal, State, Step, StepDone, StepStatus,
    Verdict,
};
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

/// What came of a step sent to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It ran, or failed: the entry the door logged (`agent=goals`), its tokens,
    /// whether it failed.
    Done(StepDone),
    /// The person said no where the door asked (the card), with why: not run.
    Denied(String),
    /// The card lapsed unanswered: no no, and not run.
    Lapsed,
    /// Not answered yet: the step waits on the person's card. It stays in flight,
    /// and the steps are asked about it again next turn (a poll, not a second
    /// send), so a card the person leaves open holds no other goal, and their
    /// pause or cancel still comes through.
    Later,
}

/// Runs a step the door let run, and says what came of it.
pub trait Steps<C> {
    fn run(&mut self, goal: u64, step: &Step<C>) -> Outcome;
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
    /// The person said no to the step on the card: it did not run.
    Denied {
        goal: u64,
        step: u32,
    },
    /// The step's card was not answered in time: it did not run, and is no no.
    Lapsed {
        goal: u64,
        step: u32,
    },
    /// The planner wrote again a step the person had said no to. It was not asked
    /// again: it is logged as held ([`crate::REPEATED`]), and the goal is paused
    /// until the person resumes it.
    Repeated {
        goal: u64,
        row: String,
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

/// Steps in a row that cannot be logged before the goal fails: a planner that
/// keeps writing them would replan and charge forever.
pub const MAX_BAD_STEPS: u32 = 3;
/// Why a step in flight with no record of its call was closed.
pub const RESTARTED: &str = "the agent restarted; the step was not run";

/// The loop's own state: the steps that asked, kept with the runner's record of
/// their call until the person answers, since a goal keeps only what it shows; how
/// many steps in a row each goal's planner wrote that could not be logged; and the
/// steps of each goal the person said no to, so none is asked a second time.
///
/// A no holds until the person acts: a repeat is logged as held with
/// [`crate::REPEATED`] and pauses the goal, and the runner forgets the no to that
/// step alone, so the person's resume lets that one step be asked once more and
/// every other refused step still needs its own. Each denied step has a record in
/// the goal's log, so a goal's list is no longer than [`crate::MAX_LOG`]. It is the
/// runner's memory only: after a restart of the agent a denied step may be asked
/// one more time.
pub struct Runner<C> {
    asked: BTreeMap<u64, Step<C>>,
    bad: BTreeMap<u64, u32>,
    denied: BTreeMap<u64, Vec<Step<C>>>,
}

impl<C> Default for Runner<C> {
    fn default() -> Self {
        Self {
            asked: BTreeMap::new(),
            bad: BTreeMap::new(),
            denied: BTreeMap::new(),
        }
    }
}

impl<C: Clone + PartialEq> Runner<C> {
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
        // A goal that ended or was forgotten has nothing left to ask.
        self.denied
            .retain(|id, _| goals.get(*id).is_some_and(|goal| !goal.state.is_final()));
        self.close_strays(goals, &mut events);
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
            // Still waiting for the person, or paused after their yes: kept, and run
            // once the goal is active again.
            if matches!(goal.state, State::Waiting | State::Paused(_)) {
                continue;
            }
            let Some(step) = self.asked.remove(&id) else {
                continue;
            };
            match (goal.flight, goal.state) {
                // A step still on the card is polled and the next is looked at.
                (Some(n), State::Active) => {
                    if self.run(goal, n, step, parts.steps, events) {
                        return true;
                    }
                }
                // The person said no where goalcore asked (`Goal::deny`): the step
                // that waited is the last one logged.
                (None, State::Active) if last_denied(goal) => self.remember_no(id, step),
                _ => {}
            }
        }
        false
    }

    /// Runs step `n` of `goal`, in flight, and records what came of it: the entry is
    /// kept whatever came of the step (#980's rule), and a no on the card is
    /// remembered. Whether it came to an end: a step not answered yet is kept, in
    /// flight, and asked about again next turn.
    fn run(
        &mut self,
        goal: &mut Goal,
        n: u32,
        step: Step<C>,
        steps: &mut dyn Steps<C>,
        events: &mut Vec<Event>,
    ) -> bool {
        match steps.run(goal.id, &step) {
            Outcome::Later => {
                self.asked.insert(goal.id, step);
                return false;
            }
            Outcome::Done(done) => {
                let entry = done.entry;
                let _ = goal.finish_step(done);
                events.push(Event::Ran {
                    goal: goal.id,
                    step: n,
                    entry,
                });
            }
            Outcome::Lapsed => {
                if goal.lapse_running().is_ok() {
                    events.push(Event::Lapsed {
                        goal: goal.id,
                        step: n,
                    });
                }
            }
            Outcome::Denied(why) => {
                if goal.deny_running(&why).is_ok() {
                    events.push(Event::Denied {
                        goal: goal.id,
                        step: n,
                    });
                    self.remember_no(goal.id, step);
                }
            }
        }
        true
    }

    fn remember_no(&mut self, goal: u64, step: Step<C>) {
        self.denied.entry(goal).or_default().push(step);
    }

    /// Whether the person said no to this very step of `goal`: the same row, paths
    /// and call. What it was expected to cost is not part of what they refused.
    fn said_no_to(&self, goal: u64, step: &Step<C>) -> bool {
        self.denied
            .get(&goal)
            .is_some_and(|denied| denied.iter().any(|no| same(no, step)))
    }

    /// Forgets the person's no to `step` alone: each refused step needs its own
    /// resume before it is asked again.
    fn forget_no(&mut self, goal: u64, step: &Step<C>) {
        if let Some(denied) = self.denied.get_mut(&goal) {
            denied.retain(|no| !same(no, step));
            if denied.is_empty() {
                self.denied.remove(&goal);
            }
        }
    }

    /// Closes as failed a step in flight that this runner has no record of (the
    /// agent restarted and reloaded the goal, then the person said yes): it never
    /// ran, and the goal goes on.
    fn close_strays(&mut self, goals: &mut Goals, events: &mut Vec<Event>) {
        let ids: Vec<u64> = goals
            .list()
            .iter()
            .filter(|goal| {
                goal.state == State::Active
                    && goal.flight.is_some()
                    && !self.asked.contains_key(&goal.id)
            })
            .map(|goal| goal.id)
            .collect();
        for id in ids {
            let Some(goal) = goals.get_mut(id) else {
                continue;
            };
            let n = goal.flight.unwrap_or(0);
            let closed = StepDone {
                entry: None,
                tokens: 0,
                failed: Some(String::from(RESTARTED)),
            };
            if goal.finish_step(closed).is_ok() {
                events.push(Event::Ran {
                    goal: id,
                    step: n,
                    entry: None,
                });
            }
        }
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
            // The budget holds for planning too: a goal past its tokens, its wall
            // time or its deadline is not planned again (its last step may still be
            // followed by the planner's "done", so the steps budget is the step's).
            if let Some(pause) = goal.planning_over(now) {
                hold(goal, pause, events);
                continue;
            }
            let (plan, tokens) = parts.planner.next(goal, now);
            goal.charge(tokens);
            if let Some(pause) = goal.planning_over(now) {
                hold(goal, pause, events);
                continue;
            }
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
        // A step the person said no to is not asked again: the goal stops here, and
        // their resume is what lets it go on.
        if self.said_no_to(id, &step) {
            let row = step.row.clone();
            match goal.hold_repeat(now, step.clone(), parts.door) {
                // Held and paused: the no to this one step is forgotten, so the
                // person's resume lets it be asked once more.
                Ok(()) => {
                    self.forget_no(id, &step);
                    events.push(Event::Repeated { goal: id, row });
                }
                Err(Refusal::LogFull) => {
                    if goal
                        .fail(now, "the goal has taken as many steps as its log holds")
                        .is_ok()
                    {
                        events.push(Event::Failed { goal: id });
                    }
                }
                // Not held (a goal this turn does not plan): the no is kept and the
                // step is not asked, so nothing reaches the card without the
                // person's act.
                Err(_) => {}
            }
            return false;
        }
        let proposed = goal.propose(now, step.clone(), person, parts.door);
        if !matches!(proposed, Err(Refusal::BadProposal(_))) {
            self.bad.remove(&id);
        }
        match proposed {
            Ok(Verdict::Run) => {
                let n = goal.flight.unwrap_or(0);
                self.run(goal, n, step, parts.steps, events)
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
            // A full log takes no more steps: the goal ends, said.
            Err(Refusal::LogFull) => {
                if goal
                    .fail(now, "the goal has taken as many steps as its log holds")
                    .is_ok()
                {
                    events.push(Event::Failed { goal: id });
                }
                false
            }
            // A step that cannot be logged, again and again: the goal ends, said.
            Err(Refusal::BadProposal(_)) => {
                let bad = self.bad.entry(id).or_default();
                *bad += 1;
                if *bad >= MAX_BAD_STEPS {
                    self.bad.remove(&id);
                    if goal
                        .fail(now, "the planner kept writing steps that cannot be logged")
                        .is_ok()
                    {
                        events.push(Event::Failed { goal: id });
                    }
                }
                false
            }
            Err(_) => false,
        }
    }
}

/// Whether `a` and `b` are the same step: row, paths and call.
fn same<C: PartialEq>(a: &Step<C>, b: &Step<C>) -> bool {
    a.row == b.row && a.targets == b.targets && a.call == b.call
}

/// Whether the last step `goal` logged is one the person said no to.
fn last_denied(goal: &Goal) -> bool {
    goal.log
        .last()
        .is_some_and(|step| step.status == StepStatus::Denied)
}

/// Pauses `goal` for `pause`, said.
fn hold(goal: &mut Goal, pause: Pause, events: &mut Vec<Event>) {
    if goal.state == State::Active {
        goal.state = State::Paused(pause);
        events.push(Event::Paused {
            goal: goal.id,
            pause,
        });
    }
}

fn waiting(goal: &Goal) -> Option<Event> {
    goal.waiting_line().map(|line| Event::Waiting {
        goal: goal.id,
        line,
    })
}

#[cfg(test)]
mod tests;
