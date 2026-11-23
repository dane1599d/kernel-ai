//! What `goals` reports to the shell for each queued command (`agent-step report
//! <n> <result>`): one shape, written and read here, so the two sides cannot
//! drift.
//!
//! ```text
//! ok                           done, with nothing more to say
//! ok LF status (LF status)*    done: where each goal the command named now stands
//!                              (`goal-list`: every goal kept)
//! gone <id>                    goals has no goal <id>
//! err <why>                    not done: why, as one printable line
//!
//! status = "goal " id " " state " " run "/" steps        goal 3 done 2/4
//! ```
//!
//! The shell acts on a status: a goal that ended loses its grants. So nothing but
//! this code's own fields may read as one:
//!
//! - A status line holds four fields and no words. A goal's text never rides in a
//!   report; the shell has it from `goal_set`.
//! - `state` is [`State::word`], and a number is written one way: digits, no sign,
//!   no leading zero.
//! - Statuses are read only under a first line that is exactly `ok`. The why of an
//!   `err` is one line ([`Report::encode`] makes it so), and is never read for one.
//! - A result with anything out of shape is refused whole: [`Report::parse`] is
//!   `None`, never the lines that did read. What reads writes back byte for byte.

use crate::goal::{Goal, State};
use crate::goals::Goals;
use crate::{MAX_GOALS, MAX_TEXT, valid};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::str::FromStr;

/// Where one goal stands, as a report says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub goal: u64,
    pub state: State,
    /// Steps that ran, and the most its budget allows.
    pub steps_run: u32,
    pub steps: u32,
}

impl Status {
    pub fn of(goal: &Goal) -> Self {
        Self {
            goal: goal.id,
            state: goal.state,
            steps_run: goal.steps_run,
            steps: goal.budget.steps,
        }
    }

    /// `goal <id> <state> <run>/<steps>`.
    pub fn line(&self) -> String {
        format!(
            "goal {} {} {}/{}",
            self.goal,
            self.state.word(),
            self.steps_run,
            self.steps
        )
    }

    /// The status `line` is, if it is exactly what [`Status::line`] writes.
    pub fn parse(line: &str) -> Option<Self> {
        let mut fields = line.strip_prefix("goal ")?.split(' ');
        let (goal, state, count) = (fields.next()?, fields.next()?, fields.next()?);
        if fields.next().is_some() {
            return None;
        }
        let (run, steps) = count.split_once('/')?;
        Some(Self {
            goal: number(goal)?,
            state: State::from_word(state)?,
            steps_run: number(run)?,
            steps: number(steps)?,
        })
    }
}

/// A number as this module writes one: digits only, with no sign and no leading
/// zero, so each has one spelling.
fn number<T: FromStr>(text: &str) -> Option<T> {
    let plain = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    match plain {
        true => text.parse().ok(),
        false => None,
    }
}

/// What came of a queued command, as `goals` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    /// Done, and where each goal the command named now stands: none, one, or for
    /// `goal-list` every goal kept.
    Ok(Vec<Status>),
    /// `goals` has no goal of this id.
    Gone(u64),
    /// Not done, with why.
    Err(String),
}

impl Report {
    /// Done, and where `goal` now stands.
    pub fn of(goal: &Goal) -> Self {
        Self::Ok(Vec::from([Status::of(goal)]))
    }

    /// `goal-list`'s answer: every goal kept.
    pub fn list(goals: &Goals) -> Self {
        Self::Ok(goals.list().iter().map(Status::of).collect())
    }

    /// The text after `agent-step report <n> `. The why of an `Err` is made one
    /// printable line of at most [`MAX_TEXT`] bytes, whatever it held.
    pub fn encode(&self) -> String {
        match self {
            Self::Ok(statuses) => statuses.iter().fold(String::from("ok"), |mut out, status| {
                out.push('\n');
                out.push_str(&status.line());
                out
            }),
            Self::Gone(goal) => format!("gone {goal}"),
            Self::Err(why) => match valid::clean(why) {
                why if why.is_empty() => String::from("err"),
                why => format!("err {why}"),
            },
        }
    }

    /// The report `result` is, if it is exactly what [`Report::encode`] writes:
    /// `None` for anything else, with no part of it read.
    pub fn parse(result: &str) -> Option<Self> {
        if result == "err" {
            return Some(Self::Err(String::new()));
        }
        if let Some(why) = result.strip_prefix("err ") {
            return valid::words_ok(why, MAX_TEXT).then(|| Self::Err(String::from(why)));
        }
        if let Some(goal) = result.strip_prefix("gone ") {
            return number(goal).map(Self::Gone);
        }
        let mut lines = result.split('\n');
        if lines.next() != Some("ok") {
            return None;
        }
        let statuses = lines.map(Status::parse).collect::<Option<Vec<Status>>>()?;
        let distinct = statuses
            .iter()
            .enumerate()
            .all(|(at, status)| statuses[..at].iter().all(|seen| seen.goal != status.goal));
        (statuses.len() <= MAX_GOALS && distinct).then_some(Self::Ok(statuses))
    }
}

#[cfg(test)]
mod tests;
