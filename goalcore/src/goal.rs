//! The goal record: what the person approved (the scope and the budget), where
//! the goal stands, and the log of every step it took.

use crate::valid;
use alloc::string::String;
use alloc::vec::Vec;

/// What a goal may use: the rows it may call and the folders it may touch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    /// Row names, each exact (`fs_list`) or a whole group (`notes_*`): the
    /// `actions` syntax of an agent's manifest (ADR 0013 section 6).
    pub rows: Vec<String>,
    /// Folders under the home folder; every path a step names must lie in one. A
    /// goal with none may touch no path: a step with a path asks.
    pub folders: Vec<String>,
}

/// What a goal may spend, approved once with the scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Steps that run, at most.
    pub steps: u32,
    /// Tokens the model and the runner count, at most.
    pub tokens: u64,
    /// Milliseconds from the goal's approval, at most.
    pub wall_ms: u64,
}

/// A row's own tier, as the shell decides it: a `Run` row runs at once, an `Ask`
/// row (no undo, or the open world) waits for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Run,
    Ask,
}

/// Whether the person is at the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Here,
    Away,
}

/// One step the goals agent proposes: one request through the one door. It holds
/// nothing the model could use to say how safe the step is: its tier is the
/// door's to decide ([`Door`]), never the model's, so a step a note or a web page
/// steered toward a row with no undo cannot arrive marked as one that runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub row: String,
    /// Every path the step acts on, from the home folder: `fs_move` has two, the
    /// one it moves and the one it moves to, and both must lie in the scope. At
    /// most [`crate::MAX_TARGETS`].
    pub targets: Vec<String>,
    /// What the step is expected to cost, in tokens.
    pub tokens_est: u64,
}

/// The one door's decision about a step's tier, which the runner supplies: the
/// registry's `agentabi::derive` of the row, or `consent::decide` on the goal's
/// approved line, with the goal's own labels, never anything the model wrote.
pub trait Door {
    fn tier(&self, row: &str, targets: &[String]) -> Tier;
}

/// A step as the log keeps it: what was proposed, and the tier the door decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub row: String,
    pub targets: Vec<String>,
    pub tier: Tier,
    pub tokens_est: u64,
}

/// Why a goal waits for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskKind {
    /// Its row asks: it has no undo.
    Tier,
    /// Its row or one of its paths is outside the approved scope: a step that
    /// needs more scope is an ask.
    Scope,
    /// The person is away and this goal does not run steps unattended.
    Away,
    /// The planner needs an answer before it can plan the next step (its
    /// `clarify`): there is no step to approve, see [`Goal::answered`].
    Question,
}

/// What a goal waits on, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub kind: AskKind,
    /// What the panel shows after "Goal '...' is waiting for you:": why the step
    /// asks, or the planner's question.
    pub why: String,
    /// The step that waits for a yes or a no; none for a question.
    pub proposal: Option<Proposal>,
}

/// What ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Steps,
    Tokens,
    Wall,
}

impl Resource {
    pub fn word(self) -> &'static str {
        match self {
            Self::Steps => "steps",
            Self::Tokens => "tokens",
            Self::Wall => "time",
        }
    }
}

/// Why a goal is paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pause {
    /// The person paused it: `goal_resume` continues.
    User,
    /// A budget is used up: it needs a raise.
    Budget(Resource),
    /// The deadline passed with the goal unfinished: it needs a later one.
    Deadline,
}

/// Where a goal stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// `goal_set` has shown the scope, budget and check-ins and waits for the yes.
    Proposed,
    /// Approved, and free to take the next step.
    Active,
    /// A step asks: see [`Goal::waiting`].
    Waiting,
    Paused(Pause),
    /// The planner says it is finished. That is its claim, not a fact: whether the
    /// goal is met is for the runner to check against the world, and to record
    /// with [`Goal::record_met`].
    Done,
    /// Given up, with the reason in [`Goal::note`].
    Failed,
    /// Stopped by the person, or never approved.
    Cancelled,
}

impl State {
    /// The goal will take no more steps.
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    /// The goal counts against the active limit: approved, and neither paused nor
    /// finished.
    pub fn is_live(self) -> bool {
        matches!(self, Self::Active | Self::Waiting)
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Paused(Pause::User) => "paused",
            Self::Paused(Pause::Budget(Resource::Steps)) => "paused-steps",
            Self::Paused(Pause::Budget(Resource::Tokens)) => "paused-tokens",
            Self::Paused(Pause::Budget(Resource::Wall)) => "paused-time",
            Self::Paused(Pause::Deadline) => "paused-deadline",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        [
            Self::Proposed,
            Self::Active,
            Self::Waiting,
            Self::Paused(Pause::User),
            Self::Paused(Pause::Budget(Resource::Steps)),
            Self::Paused(Pause::Budget(Resource::Tokens)),
            Self::Paused(Pause::Budget(Resource::Wall)),
            Self::Paused(Pause::Deadline),
            Self::Done,
            Self::Failed,
            Self::Cancelled,
        ]
        .into_iter()
        .find(|state| state.word() == word)
    }
}

/// Where one step stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepStatus {
    /// Running now.
    Running,
    /// It ran and the door answered ok.
    Ran,
    /// It ran and failed.
    Failed,
    /// It asked, and waits.
    Asked,
    /// It asked, and the person said no.
    Denied,
    /// It was held back because a budget or the deadline was reached.
    Held,
}

impl StepStatus {
    pub fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Ran => "ran",
            Self::Failed => "failed",
            Self::Asked => "asked",
            Self::Denied => "denied",
            Self::Held => "held",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        [
            Self::Running,
            Self::Ran,
            Self::Failed,
            Self::Asked,
            Self::Denied,
            Self::Held,
        ]
        .into_iter()
        .find(|status| status.word() == word)
    }
}

/// One step, as the goal's log keeps it: every step the agent proposed has one,
/// whatever came of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepLog {
    /// From 1, in the order proposed.
    pub n: u32,
    pub at_ms: u64,
    pub proposal: Proposal,
    pub status: StepStatus,
    /// The `actlog` entry the step made (under `agent=goals`), once it ran.
    pub entry: Option<u64>,
    /// Tokens the step spent.
    pub tokens: u64,
    /// Why it failed, was held, or asked; empty otherwise.
    pub note: String,
}

/// How many check-in lines of each kind have been posted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Posted {
    /// Of the set times, how many are behind us.
    pub scheduled: u32,
    /// The 75% mark has been looked at (a line posted only if the goal was behind).
    pub behind_checked: bool,
    /// The goal said it cannot finish.
    pub wont_finish: bool,
}

/// A goal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goal {
    pub id: u64,
    /// What the person asked for, in their words.
    pub text: String,
    /// The deadline, in milliseconds.
    pub by_ms: Option<u64>,
    pub scope: Scope,
    pub budget: Budget,
    /// The times the panel posts a check-in, ascending.
    pub checkins: Vec<u64>,
    /// Whether run-tier steps in scope run while the person is away (ADR 0013
    /// question 7's default: yes).
    pub away_ok: bool,
    pub state: State,
    pub made_ms: u64,
    /// When the person approved it: the budget's time runs from here.
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    /// How many steps the plan has in all, and what comes next, as the agent last
    /// said (zero and empty before it plans).
    pub planned: u32,
    pub next: String,
    /// Why it failed, or what it came to.
    pub note: String,
    /// Steps that ran (ok or not), and how many of them ok or failed.
    pub steps_run: u32,
    pub steps_ok: u32,
    pub failed: u32,
    pub denied: u32,
    /// Times the goal waited for the person: a step that asked, or a question.
    pub asked: u32,
    pub tokens_spent: u64,
    /// Whether the runner checked the goal was met, once it is [`State::Done`].
    pub met: Option<bool>,
    pub waiting: Option<Ask>,
    /// The step running now: at most one at a time.
    pub flight: Option<u32>,
    pub posted: Posted,
    pub log: Vec<StepLog>,
}

/// What `goal_set` is asked with: the request, as the person sees and approves
/// it. [`Spec::check`] holds it to the limits before it is a goal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub text: String,
    pub by_ms: Option<u64>,
    pub scope: Scope,
    pub budget: Budget,
    pub checkins: Vec<u64>,
    pub away_ok: bool,
}

impl Spec {
    /// Why this cannot be a goal made at `now`, in words for the person.
    pub fn check(&self, now: u64) -> Result<(), String> {
        valid::spec(self, now)
    }

    /// The text `goal_set` asks with: the scope, the budget and the check-ins, as
    /// the person approves them.
    pub fn approval(&self) -> String {
        valid::approval(self)
    }
}
