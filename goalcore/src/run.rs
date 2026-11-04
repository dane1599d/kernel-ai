//! The step state machine: how a goal takes a step, waits when one asks, and
//! stops at its budget and its deadline.
//!
//! ```text
//! Proposed --approve--> Active --propose--> Run ------finish_step--> Active
//!                          |  \                                         ^
//!                          |   +--ask (tier, scope, away)--> Waiting --approve (runs the one step)
//!                          |                                    \------deny---------------------^
//!                          +--budget or deadline--> Paused(..) --raise / extend / resume--> Active
//! Active --complete--> Done      any live goal --fail--> Failed     any --cancel--> Cancelled
//! ```

use crate::goal::{
    Ask, AskKind, Budget, Door, Goal, Pause, Posted, Presence, Proposal, Resource, Spec, State,
    Step, StepLog, StepStatus, Tier,
};
use crate::valid;
use crate::{MAX_LOG, MAX_STEPS, MAX_TARGETS, MAX_TOKENS, MAX_WALL_MS};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What became of a proposed step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Run it now; its step number is [`Goal::flight`].
    Run,
    /// It waits for the person: the goal is [`State::Waiting`].
    Ask(Ask),
    /// It is held back, and the goal paused, because a budget ran out or the
    /// deadline passed.
    Over(Pause),
}

/// What a step came to, as the runner reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepDone {
    /// The `actlog` entry the step made, if the door logged one.
    pub entry: Option<u64>,
    /// Tokens the step spent, as `llm` counted them.
    pub tokens: u64,
    /// Why it failed, or `None` when the door answered ok.
    pub failed: Option<String>,
}

/// Why a call on a goal did nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The goal takes no step now.
    NotActive(State),
    /// A step is running: one at a time per goal.
    StepInFlight,
    /// No step waits for an answer.
    NothingWaiting,
    /// No step is running to finish.
    NoStepRunning,
    /// The step is not in a form the log can keep.
    BadProposal(&'static str),
    /// The goal's log is full: it takes no more steps.
    LogFull,
    /// Only more budget or a later deadline resumes it.
    NeedsRaise(Pause),
    /// It is not paused.
    NotPaused(State),
    /// It was not waiting for approval.
    NotProposed(State),
    /// It is over, or never started.
    Finished(State),
    /// It is not over: only a finished goal is forgotten.
    NotFinished(State),
    /// What it waits on is a question, not a step to say yes or no to.
    NotAStep,
    /// What it waits on is a step to say yes or no to, not a question.
    NotAQuestion,
    /// Only a goal the planner finished has a "met" to record.
    NotDone(State),
    /// [`crate::MAX_ACTIVE`] goals are active: pause or finish one first.
    TooManyActive,
    /// [`crate::MAX_GOALS`] are kept: forget a finished one first.
    TooManyGoals,
    NoSuchGoal(u64),
    /// The request is not one a goal can be made from.
    BadSpec(String),
}

impl Refusal {
    /// In words for the person.
    pub fn words(&self) -> String {
        match self {
            Self::NotActive(state) => format!("the goal is {}, so it takes no step", state.word()),
            Self::StepInFlight => String::from("a step of this goal is running"),
            Self::NothingWaiting => String::from("no step of this goal is waiting for you"),
            Self::NoStepRunning => String::from("no step of this goal is running"),
            Self::BadProposal(why) => format!("that step cannot be logged: {why}"),
            Self::LogFull => String::from("the goal has taken as many steps as its log holds"),
            Self::NeedsRaise(Pause::Deadline) => {
                String::from("the deadline passed; it needs a later one")
            }
            Self::NeedsRaise(_) => String::from("it used its budget; it needs more"),
            Self::NotPaused(state) => format!("the goal is {}, not paused", state.word()),
            Self::NotProposed(state) => {
                format!("the goal is {}, not waiting for your yes", state.word())
            }
            Self::Finished(state) => format!("the goal is {}", state.word()),
            Self::NotFinished(state) => {
                format!(
                    "the goal is {}, not finished; cancel it first",
                    state.word()
                )
            }
            Self::TooManyActive => format!(
                "{} goals are active; pause or finish one first",
                crate::MAX_ACTIVE
            ),
            Self::TooManyGoals => format!(
                "{} goals are kept; forget a finished one first",
                crate::MAX_GOALS
            ),
            Self::NotAStep => String::from("the goal asks a question; answer it"),
            Self::NotAQuestion => String::from("the goal waits for a yes or no to a step"),
            Self::NotDone(state) => format!("the goal is {}, not done", state.word()),
            Self::NoSuchGoal(id) => format!("there is no goal #{id}"),
            Self::BadSpec(why) => why.clone(),
        }
    }
}

fn held_note(pause: Pause) -> String {
    match pause {
        Pause::Deadline => String::from("the deadline passed"),
        Pause::Budget(resource) => format!("the {} budget is used up", resource.word()),
        Pause::User => String::from("paused"),
    }
}

/// A step the log can keep: a row name, and printable paths.
fn step_ok<C>(step: &Step<C>) -> Result<(), Refusal> {
    if !valid::row_name_ok(&step.row) {
        return Err(Refusal::BadProposal("the row is not a row name"));
    }
    if step.targets.len() > MAX_TARGETS {
        return Err(Refusal::BadProposal("the step names too many paths"));
    }
    for path in &step.targets {
        if path.is_empty() || path.len() > 255 {
            return Err(Refusal::BadProposal("a path is empty or too long"));
        }
        if !valid::printable(path) {
            return Err(Refusal::BadProposal(
                "a path has a character that is not printable",
            ));
        }
    }
    Ok(())
}

/// The paths of a step in words: `reports/a.txt` or `inbox/a.txt, archive/a.txt`.
pub(crate) fn paths(targets: &[String]) -> String {
    let mut out = String::new();
    for (at, path) in targets.iter().enumerate() {
        if at > 0 {
            out.push_str(", ");
        }
        out.push_str(path);
    }
    out
}

impl Goal {
    pub(crate) fn from_spec(id: u64, spec: Spec, now: u64) -> Self {
        Self {
            id,
            text: spec.text,
            by_ms: spec.by_ms,
            scope: spec.scope,
            budget: spec.budget,
            checkins: spec.checkins,
            away_ok: spec.away_ok,
            state: State::Proposed,
            made_ms: now,
            started_ms: None,
            ended_ms: None,
            planned: 0,
            next: String::new(),
            note: String::new(),
            steps_run: 0,
            steps_ok: 0,
            failed: 0,
            denied: 0,
            asked: 0,
            tokens_spent: 0,
            met: None,
            waiting: None,
            flight: None,
            posted: Posted::default(),
            log: Vec::new(),
        }
    }

    /// The person said yes to the scope, budget and check-ins: the goal runs, and
    /// its budget's time starts. The active limit is [`crate::Goals::approve`]'s.
    pub(crate) fn start(&mut self, now: u64) -> Result<(), Refusal> {
        if self.state != State::Proposed {
            return Err(Refusal::NotProposed(self.state));
        }
        self.state = State::Active;
        self.started_ms = Some(now);
        Ok(())
    }

    /// Whether a step that costs about `est` tokens would run past a budget or the
    /// deadline at `now`.
    fn over(&self, now: u64, est: u64) -> Option<Pause> {
        let started = self.started_ms.unwrap_or(now);
        if self.by_ms.is_some_and(|by| now >= by) {
            Some(Pause::Deadline)
        } else if now.saturating_sub(started) >= self.budget.wall_ms {
            Some(Pause::Budget(Resource::Wall))
        } else if self.steps_run >= self.budget.steps {
            Some(Pause::Budget(Resource::Steps))
        } else if self.tokens_spent.saturating_add(est) > self.budget.tokens {
            Some(Pause::Budget(Resource::Tokens))
        } else {
            None
        }
    }

    /// Why this step has to ask, if it does. Scope first: a step that needs more
    /// scope than was approved is an ask however safe its row is.
    fn ask_for(
        &self,
        p: &Proposal,
        reason: Option<String>,
        person: Presence,
    ) -> Option<(AskKind, String)> {
        if !valid::row_in(&self.scope.rows, &p.row) {
            let why = format!("{} is outside what you approved", p.row);
            return Some((AskKind::Scope, valid::clean(&why)));
        }
        if let Some(path) = p
            .targets
            .iter()
            .find(|path| !valid::path_in(&self.scope.folders, path))
        {
            let why = format!("{path} is outside the folders you approved");
            return Some((AskKind::Scope, valid::clean(&why)));
        }
        if p.tier == Tier::Ask {
            let why = reason.unwrap_or_else(|| format!("{} has no undo, so it asks", p.row));
            return Some((AskKind::Tier, valid::clean(&why)));
        }
        if person == Presence::Away && !self.away_ok {
            let why = format!("{} waits until you are back", p.row);
            return Some((AskKind::Away, valid::clean(&why)));
        }
        None
    }

    fn push_log(&mut self, p: Proposal, now: u64, status: StepStatus, note: String) {
        let n = self.log.len() as u32 + 1;
        self.log.push(StepLog {
            n,
            at_ms: now,
            proposal: p,
            status,
            entry: None,
            tokens: 0,
            note,
        });
    }

    /// The agent proposes its next step. A run-tier step in scope with budget left
    /// runs ([`Verdict::Run`]; report it with [`Goal::finish_step`]); one that asks,
    /// or needs more scope, waits for the person ([`Verdict::Ask`]); one past the
    /// budget or the deadline pauses the goal ([`Verdict::Over`]). Every proposal
    /// that is not refused outright is logged.
    ///
    /// The step carries no tier: `door` decides it from the whole step, `call`
    /// included, so nothing the model wrote can mark a step with no undo, or one
    /// that content steers, as one that runs. `C` is the runner's record of the
    /// call (see [`Step`]).
    pub fn propose<C>(
        &mut self,
        now: u64,
        step: Step<C>,
        person: Presence,
        door: &dyn Door<C>,
    ) -> Result<Verdict, Refusal> {
        if self.state != State::Active {
            return Err(Refusal::NotActive(self.state));
        }
        if self.flight.is_some() {
            return Err(Refusal::StepInFlight);
        }
        if self.log.len() >= MAX_LOG {
            return Err(Refusal::LogFull);
        }
        step_ok(&step)?;
        let tier = door.tier(&step);
        let reason = match tier {
            Tier::Ask => door.reason(&step),
            Tier::Run => None,
        };
        let p = Proposal {
            tier,
            row: step.row,
            targets: step.targets,
            tokens_est: step.tokens_est,
        };
        if let Some(pause) = self.over(now, p.tokens_est) {
            self.state = State::Paused(pause);
            self.push_log(p, now, StepStatus::Held, held_note(pause));
            return Ok(Verdict::Over(pause));
        }
        let n = self.log.len() as u32 + 1;
        match self.ask_for(&p, reason, person) {
            Some((kind, why)) => {
                let ask = Ask {
                    kind,
                    why: why.clone(),
                    proposal: Some(p.clone()),
                };
                self.state = State::Waiting;
                self.asked += 1;
                self.waiting = Some(ask.clone());
                self.push_log(p, now, StepStatus::Asked, why);
                Ok(Verdict::Ask(ask))
            }
            None => {
                self.flight = Some(n);
                self.push_log(p, now, StepStatus::Running, String::new());
                Ok(Verdict::Run)
            }
        }
    }

    /// The step that waits, the last one logged: nothing else is proposed while a
    /// goal waits.
    fn last_step(&mut self) -> Option<&mut StepLog> {
        self.log.last_mut()
    }

    /// The person said yes to the step that waits: it runs, this once. The scope
    /// stays as it was approved, so another step outside it asks again. If a budget
    /// ran out while it waited, the goal pauses instead ([`Verdict::Over`]).
    ///
    /// **Only the trusted path's answer to the ask, its ONCE checked (ADR 0002),
    /// may call this or [`Goal::deny`]**: nothing a model, an app or a page wrote
    /// is a yes. goalcore cannot tell who is calling.
    pub fn approve(&mut self, now: u64) -> Result<Verdict, Refusal> {
        let tokens_est = match (&self.state, &self.waiting) {
            (
                State::Waiting,
                Some(Ask {
                    proposal: Some(p), ..
                }),
            ) => p.tokens_est,
            (State::Waiting, Some(_)) => return Err(Refusal::NotAStep),
            _ => return Err(Refusal::NothingWaiting),
        };
        let Some(ask) = self.waiting.take() else {
            return Err(Refusal::NothingWaiting);
        };
        if let Some(pause) = self.over(now, tokens_est) {
            self.state = State::Paused(pause);
            let note = held_note(pause);
            if let Some(step) = self.last_step() {
                step.status = StepStatus::Held;
                step.note = note;
            }
            return Ok(Verdict::Over(pause));
        }
        let note = valid::clean(&format!("approved: {}", ask.why));
        let n = match self.last_step() {
            Some(step) => {
                step.status = StepStatus::Running;
                step.note = note;
                step.n
            }
            None => return Err(Refusal::NothingWaiting),
        };
        self.state = State::Active;
        self.flight = Some(n);
        Ok(Verdict::Run)
    }

    /// The person said no to the step that waits: it is dropped, and the goal is
    /// free to propose another. Only the trusted path's answer may call it, as
    /// for [`Goal::approve`].
    pub fn deny(&mut self) -> Result<(), Refusal> {
        match (&self.state, &self.waiting) {
            (
                State::Waiting,
                Some(Ask {
                    proposal: Some(_), ..
                }),
            ) => {}
            (State::Waiting, Some(_)) => return Err(Refusal::NotAStep),
            _ => return Err(Refusal::NothingWaiting),
        }
        self.waiting = None;
        if let Some(step) = self.last_step() {
            step.status = StepStatus::Denied;
        }
        self.denied += 1;
        self.state = State::Active;
        Ok(())
    }

    /// The planner needs an answer before it can plan the next step (its
    /// `clarify`): the goal waits, and the panel shows the question. It counts as
    /// a time the goal waited for the person ([`Goal::asked`]). It is not a step,
    /// so it is not logged as one.
    pub fn clarify(&mut self, question: &str) -> Result<(), Refusal> {
        if self.state != State::Active {
            return Err(Refusal::NotActive(self.state));
        }
        if self.flight.is_some() {
            return Err(Refusal::StepInFlight);
        }
        let why = match valid::clean(question) {
            empty if empty.is_empty() => String::from("the agent has a question"),
            why => why,
        };
        self.waiting = Some(Ask {
            kind: AskKind::Question,
            why,
            proposal: None,
        });
        self.state = State::Waiting;
        self.asked += 1;
        Ok(())
    }

    /// The person answered the planner's question (the runner gives the answer to
    /// the planner): the goal is free to plan its next step.
    pub fn answered(&mut self) -> Result<(), Refusal> {
        match (&self.state, &self.waiting) {
            (State::Waiting, Some(Ask { proposal: None, .. })) => {
                self.waiting = None;
                self.state = State::Active;
                Ok(())
            }
            (State::Waiting, Some(_)) => Err(Refusal::NotAQuestion),
            _ => Err(Refusal::NothingWaiting),
        }
    }

    /// The runner checked the world and says whether the goal that the planner
    /// called done is met: `Done` is the planner's claim, this is the check.
    pub fn record_met(&mut self, met: bool) -> Result<(), Refusal> {
        if self.state != State::Done {
            return Err(Refusal::NotDone(self.state));
        }
        self.met = Some(met);
        Ok(())
    }

    /// The running step ended: what it cost, the entry it made, and whether the
    /// door answered ok. It counts against the budget either way.
    pub fn finish_step(&mut self, done: StepDone) -> Result<(), Refusal> {
        let Some(n) = self.flight.take() else {
            return Err(Refusal::NoStepRunning);
        };
        self.tokens_spent = self.tokens_spent.saturating_add(done.tokens);
        self.steps_run += 1;
        let (status, note) = match done.failed {
            None => {
                self.steps_ok += 1;
                (StepStatus::Ran, None)
            }
            Some(why) => {
                self.failed += 1;
                (StepStatus::Failed, Some(valid::clean(&why)))
            }
        };
        if let Some(step) = self.log.iter_mut().rev().find(|s| s.n == n) {
            step.status = status;
            step.entry = done.entry;
            step.tokens = done.tokens;
            if let Some(note) = note {
                step.note = note;
            }
        }
        Ok(())
    }

    /// Tokens spent outside a step: the planner's own prompts, as `llm` counts them.
    pub fn charge(&mut self, tokens: u64) {
        self.tokens_spent = self.tokens_spent.saturating_add(tokens);
    }

    /// The agent's plan: how many steps it has in all, and what comes next.
    pub fn plan(&mut self, planned: u32, next: &str) {
        self.planned = planned.min(MAX_STEPS);
        self.next = valid::clean(next);
    }

    pub(crate) fn running(&self) -> bool {
        matches!(
            self.state,
            State::Active | State::Waiting | State::Paused(_)
        )
    }

    /// Ends the goal in `state`, closing the step that was running or waiting.
    fn end(&mut self, state: State, now: u64, note: &str) {
        let closing = match self.waiting.take() {
            Some(Ask {
                proposal: Some(_), ..
            }) => Some(StepStatus::Held),
            // A question is no step: nothing in the log to close.
            Some(_) => None,
            None => self.flight.take().map(|_| StepStatus::Failed),
        };
        if let (Some(status), Some(step)) = (closing, self.last_step()) {
            step.status = status;
            step.note = String::from("the goal ended");
        }
        self.flight = None;
        self.state = state;
        self.ended_ms = Some(now);
        self.note = valid::clean(note);
    }

    /// The goal is met. The agent says so after a step, with no step running.
    pub fn complete(&mut self, now: u64, note: &str) -> Result<(), Refusal> {
        if self.state != State::Active {
            return Err(Refusal::NotActive(self.state));
        }
        if self.flight.is_some() {
            return Err(Refusal::StepInFlight);
        }
        self.end(State::Done, now, note);
        Ok(())
    }

    /// The agent gives up, with the reason.
    pub fn fail(&mut self, now: u64, why: &str) -> Result<(), Refusal> {
        if !self.running() {
            return Err(Refusal::Finished(self.state));
        }
        self.end(State::Failed, now, why);
        Ok(())
    }

    /// The person stops the goal, or declines it before it starts. What it did
    /// stays done: [`Goal::undo_entries`] says what an undo of it takes.
    pub fn cancel(&mut self, now: u64) -> Result<(), Refusal> {
        if !(self.running() || self.state == State::Proposed) {
            return Err(Refusal::Finished(self.state));
        }
        self.end(State::Cancelled, now, "stopped");
        Ok(())
    }

    /// The person pauses it. A step already running finishes; no other starts.
    pub fn pause(&mut self) -> Result<(), Refusal> {
        if self.state != State::Active {
            return Err(Refusal::NotActive(self.state));
        }
        self.state = State::Paused(Pause::User);
        Ok(())
    }

    /// The person resumes a goal they paused. A goal paused by its budget or its
    /// deadline needs [`Goal::raise`] or [`Goal::extend`] first.
    pub(crate) fn resume(&mut self) -> Result<(), Refusal> {
        match self.state {
            State::Paused(Pause::User) => {
                self.state = State::Active;
                Ok(())
            }
            State::Paused(pause) => Err(Refusal::NeedsRaise(pause)),
            other => Err(Refusal::NotPaused(other)),
        }
    }

    /// The budget the goal asked for more of, now approved: adds `more` to each
    /// limit, and resumes the goal if it has room again. A goal that is still out
    /// of something stays paused, for what it lacks.
    pub(crate) fn raise(&mut self, now: u64, more: Budget) -> Result<State, Refusal> {
        let State::Paused(Pause::Budget(_)) = self.state else {
            return Err(match self.state {
                State::Paused(pause) => Refusal::NeedsRaise(pause),
                other => Refusal::NotPaused(other),
            });
        };
        self.budget = Budget {
            steps: self.budget.steps.saturating_add(more.steps).min(MAX_STEPS),
            tokens: self
                .budget
                .tokens
                .saturating_add(more.tokens)
                .min(MAX_TOKENS),
            wall_ms: self
                .budget
                .wall_ms
                .saturating_add(more.wall_ms)
                .min(MAX_WALL_MS),
        };
        self.state = match self.over(now, 1) {
            Some(pause) => State::Paused(pause),
            None => State::Active,
        };
        Ok(self.state)
    }

    /// A later deadline, approved: resumes a goal the deadline paused, if it has
    /// room otherwise.
    pub(crate) fn extend(&mut self, now: u64, by_ms: u64) -> Result<State, Refusal> {
        if self.state != State::Paused(Pause::Deadline) {
            return Err(match self.state {
                State::Paused(pause) => Refusal::NeedsRaise(pause),
                other => Refusal::NotPaused(other),
            });
        }
        if by_ms <= now || self.by_ms.is_some_and(|old| by_ms <= old) {
            return Err(Refusal::BadSpec(String::from(
                "the new deadline is not later than now and the old one",
            )));
        }
        self.by_ms = Some(by_ms);
        self.state = match self.over(now, 1) {
            Some(pause) => State::Paused(pause),
            None => State::Active,
        };
        Ok(self.state)
    }

    /// The `actlog` entries the goal's steps made, newest first: what `undo goals`
    /// takes one at a time, and what cancelling the goal offers to undo.
    pub fn undo_entries(&self) -> Vec<u64> {
        self.log
            .iter()
            .rev()
            .filter_map(|step| step.entry)
            .collect()
    }

    /// The panel's line while a step waits: "Goal '...' is waiting for you: why".
    pub fn waiting_line(&self) -> Option<String> {
        let ask = self.waiting.as_ref()?;
        Some(format!(
            "Goal '{}' is waiting for you: {}",
            self.text, ask.why
        ))
    }
}
