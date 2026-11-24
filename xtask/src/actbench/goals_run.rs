//! Arm `goals` through the OS's own parts (row G1's gate, `docs/plans/agents-os.md`):
//! S13-5's goals, each pursued by `goalcore::runner` as the `goals` agent runs it,
//! with each step admitted by agentid's door under the goal's ceiling.
//!
//! - **Approval.** The person's yes to `goal_set` is `Roster::grant_goal` from
//!   `kai` with the person's mark, holding the goal's scope rows. A scope that
//!   holds a row a goal may never hold (an install, a send, a permission change) is
//!   refused there, and the goal never starts: it counts as not met
//!   ([`Run::refused`]).
//! - **Planner.** Each plan is one request through llm's `act` path, as before: the
//!   goal and what is done as the line, the step count as the State line, a read's
//!   answer as the next request's `read`. `say` is the goal met, `clarify` a
//!   check-in that ends it, and a `script` spends a step and asks nothing.
//! - **Door.** A step's tier is the row's derived tier (`agentabi::derive`).
//! - **Steps.** A step that may run is admitted with `Roster::admit_for_goal`, then
//!   applied to the mock OS. A step outside the scope never reaches it: goalcore
//!   asks the person about it, and the person says no.
//! - **The person.** On goalcore's ask: a step outside the scope is refused, a
//!   step whose row asks is approved; each ask is a check-in.
//!
//! The budget is the goal's: model calls, whatever came of them.

use super::goals::{Effect, Goal, Run, World, asks, line, path_arg, progressed, shown, state};
use agentid::{Builtin, Caller, Mark, Roster, Source};
use goalcore::runner::{Event, Outcome, Parts, Plan, Planner, Runner, Steps};
use goalcore::{AskKind, Budget, Door, Goals, Presence, Scope, Spec, State, Step, StepDone, Tier};
use llmproto::act::{ActRequest, Call, Read};
use std::cell::RefCell;

/// The one folder a goal's scope names: the mock home folder, where every goal's
/// files are. A path `notes.txt` is `home/notes.txt` to goalcore.
const HOME: &str = "home";
/// The clock goals start on, in ms.
const T0: u64 = 1_000_000;
/// Tokens a goal may spend: far more than a budget of steps uses.
const TOKENS: u64 = 1_000_000;
/// The time llm gives one step.
const TIMEOUT_MS: u64 = 60_000;

/// What the planner, the steps and the person share while a goal runs.
struct Shared<'g> {
    goal: &'g Goal,
    world: World,
    progress: String,
    read: Option<Read>,
    run: Run,
}

/// The model, asked for each step.
struct Model<'s, 'g, F> {
    shared: &'s RefCell<Shared<'g>>,
    next: F,
}

impl<F: FnMut(&ActRequest) -> Result<Call, String>> Planner<Call> for Model<'_, '_, F> {
    fn next(&mut self, _goal: &goalcore::Goal, _now: u64) -> (Plan<Call>, u64) {
        let request = {
            let mut shared = self.shared.borrow_mut();
            let shared = &mut *shared;
            shared.run.steps += 1;
            ActRequest {
                line: line(shared.goal.text, &shared.progress),
                state: state(shared.run.steps, shared.goal.budget),
                timeout_ms: TIMEOUT_MS,
                read: shared.read.take(),
                apps: String::new(),
            }
        };
        let call = match (self.next)(&request) {
            Ok(call) => call,
            Err(why) => {
                let mut shared = self.shared.borrow_mut();
                shared.run.calls.push(format!("error: {why}"));
                return (Plan::Fail(why), 0);
            }
        };
        let mut shared = self.shared.borrow_mut();
        shared.run.calls.push(call.text());
        let plan = match call.name.as_str() {
            "say" => Plan::Done(String::from("the model says the goal is met")),
            "clarify" => {
                shared.run.check_ins += 1;
                Plan::Fail(String::from("the model asked a question"))
            }
            // A script cannot act inside a goal's scope: the step is spent, the
            // model told, and nothing asks.
            "script" => {
                shared.run.scripts += 1;
                let said = "not used in a goal; call one of the goal's actions";
                shared.progress = progressed(&shared.progress, "script", said);
                Plan::Later
            }
            _ => Plan::Step(Step {
                row: call.name.clone(),
                targets: targets(&call),
                tokens_est: 0,
                call,
            }),
        };
        (plan, 0)
    }
}

/// Every path a call names, as goalcore checks them against the scope.
fn targets(call: &Call) -> Vec<String> {
    ["path", "from", "to"]
        .into_iter()
        .filter_map(|key| path_arg(call, key))
        .map(|path| match path.as_str() {
            "" | "." | "~" => String::from(HOME),
            path => format!("{HOME}/{path}"),
        })
        .collect()
}

/// The one door's tier for a step: its row's derived tier.
struct RowTier;

impl Door<Call> for RowTier {
    fn tier(&self, step: &Step<Call>) -> Tier {
        match asks(&step.row) {
            true => Tier::Ask,
            false => Tier::Run,
        }
    }
}

/// The shell's door for the agent's steps: admitted under the goal's ceiling, then
/// applied to the mock OS.
struct Shell<'s, 'g> {
    shared: &'s RefCell<Shared<'g>>,
    roster: &'s Roster,
    goals: Caller,
    entries: u64,
}

impl Steps<Call> for Shell<'_, '_> {
    fn run(&mut self, goal: u64, step: &Step<Call>) -> Outcome {
        let mut shared = self.shared.borrow_mut();
        let shared = &mut *shared;
        let call = &step.call;
        if let Err(why) = self.roster.admit_for_goal(&self.goals, goal, &step.row) {
            let said = format!("refused: {why:?}");
            shared.progress = progressed(&shared.progress, &shown(call), &said);
            return Outcome::Done(StepDone {
                entry: None,
                tokens: 0,
                failed: Some(said),
            });
        }
        if !shared.goal.scope.contains(&step.row.as_str()) {
            shared.run.ran_outside += 1;
        }
        let (said, failed) = match shared.world.apply(call) {
            Effect::Ran { said, data } => {
                if let Some(data) = data {
                    shared.read = Some(Read {
                        call: call.text(),
                        data,
                    });
                }
                (said, None)
            }
            Effect::Failed(why) => (format!("failed: {why}"), Some(why)),
        };
        shared.progress = progressed(&shared.progress, &shown(call), &said);
        self.entries += 1;
        Outcome::Done(StepDone {
            entry: Some(self.entries),
            tokens: 0,
            failed,
        })
    }
}

/// The goal as `goal_set` makes it: its words, its scope's rows in the one home
/// folder, and its step budget.
fn spec(goal: &Goal) -> Spec {
    Spec {
        text: String::from(goal.text),
        by_ms: None,
        scope: Scope {
            rows: goal.scope.iter().map(|row| String::from(*row)).collect(),
            folders: vec![String::from(HOME)],
        },
        budget: Budget {
            steps: goal.budget as u32,
            tokens: TOKENS,
            wall_ms: 24 * 60 * 60 * 1000,
        },
        checkins: Vec::new(),
        away_ok: true,
    }
}

/// Runs `goal` through goalcore's runner and agentid's door, asking `next` for
/// each step's call.
pub fn pursue(goal: &Goal, next: impl FnMut(&ActRequest) -> Result<Call, String>) -> Run {
    let shared = RefCell::new(Shared {
        goal,
        world: goal.start(),
        progress: String::new(),
        read: None,
        run: Run::default(),
    });
    let mut goals = Goals::new();
    let mut roster = Roster::new();
    let (Some(kai), Some(agent)) = (
        builtin(&roster, Builtin::Kai),
        builtin(&roster, Builtin::Goals),
    ) else {
        return refused(shared, "the roster has no kai or goals");
    };
    let id = match goals.set(spec(goal), T0) {
        Ok(id) => id,
        Err(why) => return refused(shared, &why.words()),
    };
    let scope: Vec<String> = goal.scope.iter().map(|row| String::from(*row)).collect();
    // SAFETY: the bench stands in for the console, where the person answers
    // `goal_set`'s ask with their own keys.
    let person = Mark::of(unsafe { Source::hardware() });
    if let Err(why) = roster.grant_goal(&kai, person, id, &scope, &agentabi::ROWS) {
        return refused(shared, &format!("{why:?}"));
    }
    if goals.approve(id, T0).is_err() {
        return refused(shared, "goalcore did not start the goal");
    }
    let mut model = Model {
        shared: &shared,
        next,
    };
    let mut shell = Shell {
        shared: &shared,
        roster: &roster,
        goals: agent,
        entries: 0,
    };
    let mut runner = Runner::new();
    let turns = goal.budget * 4 + 4;
    for turn in 0..turns {
        let spent = shared.borrow().run.steps >= goal.budget;
        let met = (goal.met)(&shared.borrow().world);
        let live = goals
            .get(id)
            .is_some_and(|g| matches!(g.state, State::Active | State::Waiting));
        if spent || met || !live {
            break;
        }
        let now = T0 + 1 + turn as u64;
        let mut parts = Parts {
            planner: &mut model,
            door: &RowTier,
            steps: &mut shell,
        };
        for event in runner.turn(&mut goals, now, Presence::Here, &mut parts) {
            if let Event::Waiting { goal, .. } = event {
                answer(&mut goals, goal, now, &shared);
            }
        }
    }
    drop((model, shell));
    let Shared { world, mut run, .. } = shared.into_inner();
    run.met = (goal.met)(&world);
    run
}

/// The person's answer to goalcore's ask: no to a step outside the scope, yes to
/// one whose row asks. Each is a check-in.
fn answer(goals: &mut Goals, id: u64, now: u64, shared: &RefCell<Shared<'_>>) {
    let Some(goal) = goals.get_mut(id) else {
        return;
    };
    let Some(ask) = goal.waiting.clone() else {
        return;
    };
    let mut shared = shared.borrow_mut();
    let shared = &mut *shared;
    shared.run.check_ins += 1;
    match ask.kind {
        AskKind::Scope => {
            shared.run.outside += 1;
            let row = ask.proposal.map(|p| p.row).unwrap_or_default();
            let said = format!("{row} not allowed: outside the goal");
            shared.progress = progressed(&shared.progress, &row, &said);
            let _ = goal.deny();
        }
        AskKind::Tier | AskKind::Away => {
            let _ = goal.approve(now);
        }
        AskKind::Question => {
            let _ = goal.cancel(now);
        }
    }
}

/// The built-in agent `which`'s caller: the built-ins hold the first badges, in
/// [`Builtin::ALL`]'s order, and the name is checked.
fn builtin(roster: &Roster, which: Builtin) -> Option<Caller> {
    let at = Builtin::ALL.iter().position(|b| *b == which)?;
    let caller = roster.caller(at as u32 + 1).ok()?;
    (caller.name().as_str() == which.name()).then_some(caller)
}

/// A goal that never started: refused at approval, why recorded.
fn refused(shared: RefCell<Shared<'_>>, why: &str) -> Run {
    let mut run = shared.into_inner().run;
    run.refused = true;
    run.calls.push(format!("refused at approval: {why}"));
    run
}
