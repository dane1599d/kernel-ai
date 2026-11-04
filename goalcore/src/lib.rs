//! Proactive goals (ADR 0013 section 5): a goal is a standing request with a
//! deadline, a budget and check-ins, run by the `goals` agent as steps through the
//! one door, under its own name, while the person may be away.
//!
//! This crate is the part with no OS in it: the goal record, the step state
//! machine, the check-in rules and the step log. The runner drives it with a clock
//! it passes in (`now`, in milliseconds), so every rule here is tested with a fake
//! one. What it decides, and the runner does:
//!
//! - **Approval.** [`Spec::approval`] is what `goal_set` asks with: the scope (the
//!   rows and folders the goal may use), the budget (steps, tokens, wall time) and
//!   the check-in times. Only an approved goal runs ([`Goals::approve`]), and only
//!   inside what was approved.
//! - **Steps.** One at a time per goal. [`Goal::propose`] says whether a step runs,
//!   asks, or is held back by the budget or the deadline. A run-tier step inside
//!   the scope runs even while the person is away (question 7's default, a flag
//!   per goal turns it off). A step that asks, or needs more scope than was
//!   approved, pauses the goal ([`State::Waiting`]) until the person answers:
//!   [`Goal::approve`] runs that one step, with the scope unchanged, and
//!   [`Goal::deny`] drops it. Nothing outside the scope runs unasked.
//! - **Budget.** At the budget the goal pauses and asks for more
//!   ([`Goals::raise`]); at the deadline it pauses and asks for a later one
//!   ([`Goals::extend`]).
//! - **Check-ins.** At the times set, at 75% of the time to the deadline if the
//!   goal is behind, and as soon as it cannot finish ([`Goal::due_checkin`]): one
//!   line each, saying what is done, what is next and what it waits on.
//! - **Log.** Every step proposed has a record ([`StepLog`]), with the `actlog`
//!   entry it made, so `undo goals` and a goal's cancel know what to undo
//!   ([`Goal::undo_entries`]).
//! - **Storm rules.** At most [`MAX_ACTIVE`] goals active at once, at most
//!   [`MAX_GOALS`] kept, one step at a time per goal.
//!
//! The text form, `kai-goals 1` ([`Goals::to_text`], [`Goals::parse`]), is read as
//! hostile, as a file that comes back from disk: anything out of shape refuses the
//! whole file with its line, and what reads writes back byte for byte.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod checkin;
mod goal;
mod goals;
mod run;
mod text;
mod valid;

#[cfg(test)]
mod tests;

pub use checkin::{CheckIn, CheckKind};
pub use goal::{
    Ask, AskKind, Budget, Goal, Pause, Posted, Presence, Proposal, Resource, Scope, Spec, State,
    StepLog, StepStatus, Tier,
};
pub use goals::Goals;
pub use run::{Refusal, StepDone, Verdict};
pub use text::MAGIC;

/// Longest text of a goal, a note or a step's note.
pub const MAX_TEXT: usize = 200;
/// Most rows a scope names, and most folders.
pub const MAX_ROWS: usize = 16;
pub const MAX_FOLDERS: usize = 8;
/// Longest row name.
pub const ROW_CHARS: usize = 48;
/// Most check-in times a goal sets.
pub const MAX_CHECKINS: usize = 8;
/// Most steps a goal's budget may allow.
pub const MAX_STEPS: u32 = 64;
/// Most tokens a goal's budget may allow.
pub const MAX_TOKENS: u64 = 10_000_000;
/// Longest a goal may take: 30 days.
pub const MAX_WALL_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// Most steps one goal's log holds; a goal that reaches it takes no more.
pub const MAX_LOG: usize = 128;
/// Goals kept, finished ones included: forget one to make room.
pub const MAX_GOALS: usize = 16;
/// Goals active (approved and neither paused nor finished) at once: the storm
/// rule (est., ADR 0013 section 5).
pub const MAX_ACTIVE: usize = 4;
