//! Check-in rules (ADR 0013 section 5): at the times the person set, at 75% of the
//! time to the deadline if the goal is behind, and as soon as it cannot finish, so
//! it says so before its deadline, not after. Each is one line for the panel: what
//! is done, what is next, what it waits on.

use crate::goal::{Goal, Pause, State};
use alloc::format;
use alloc::string::String;

/// Why a check-in was posted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckKind {
    /// A time the person set has come.
    Scheduled,
    /// 75% of the time to the deadline has gone, and the goal is behind.
    Behind,
    /// The goal cannot finish within its steps, tokens or time.
    WontFinish,
}

/// One line for the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckIn {
    pub kind: CheckKind,
    pub line: String,
}

/// `a` over `b`, rounded up; `b` is not zero.
fn div_ceil(a: u64, b: u64) -> u64 {
    a / b + u64::from(!a.is_multiple_of(b))
}

impl Goal {
    /// "Goal '...': 2 of 4 steps done; next: ...; waiting on: ..."
    pub fn progress_line(&self) -> String {
        let done = match self.planned {
            0 => format!("{} steps done", self.steps_ok),
            planned => format!("{} of {planned} steps done", self.steps_ok),
        };
        let next = match (&self.waiting, self.next.is_empty()) {
            (Some(ask), _) => match &ask.proposal.target {
                Some(target) => format!("{} {target}", ask.proposal.row),
                None => ask.proposal.row.clone(),
            },
            (None, true) => String::from("not planned yet"),
            (None, false) => self.next.clone(),
        };
        let waiting = match (&self.waiting, self.state) {
            (Some(ask), _) => ask.why.clone(),
            (None, State::Paused(Pause::User)) => String::from("you, who paused it"),
            (None, State::Paused(Pause::Budget(resource))) => {
                format!("more {} from you", resource.word())
            }
            (None, State::Paused(Pause::Deadline)) => String::from("a later deadline from you"),
            (None, _) => String::from("nothing"),
        };
        format!(
            "Goal '{}': {done}; next: {next}; waiting on: {waiting}",
            self.text
        )
    }

    /// Share of the time between approval and the deadline that has gone, in
    /// percent, when there is a deadline.
    fn elapsed_pct(&self, now: u64) -> Option<u64> {
        let (by, started) = (self.by_ms?, self.started_ms?);
        let span = by.saturating_sub(started).max(1);
        let gone = u128::from(now.saturating_sub(started)) * 100 / u128::from(span);
        Some(gone.min(100) as u64)
    }

    /// The moment 75% of the time to the deadline has gone.
    fn behind_mark(&self) -> Option<u64> {
        let (by, started) = (self.by_ms?, self.started_ms?);
        let span = u128::from(by.saturating_sub(started));
        Some(started.saturating_add((span * 3 / 4) as u64))
    }

    /// Behind: less of the plan is done than of the time. A goal with no plan yet
    /// is behind once any time has gone.
    fn is_behind(&self, now: u64) -> bool {
        let Some(gone) = self.elapsed_pct(now) else {
            return false;
        };
        match self.planned {
            0 => gone > 0,
            planned => u64::from(self.steps_ok) * 100 < u64::from(planned) * gone,
        }
    }

    /// Why the plan cannot finish in what is left, from how the steps so far went.
    /// Nothing is said before a step has run, since there is no rate to go by,
    /// unless the plan alone is longer than the steps that remain.
    fn cannot_finish(&self, now: u64) -> Option<String> {
        let remaining = u64::from(self.planned.saturating_sub(self.steps_ok));
        if self.planned == 0 || remaining == 0 {
            return None;
        }
        let steps_left = u64::from(self.budget.steps.saturating_sub(self.steps_run));
        if remaining > steps_left {
            return Some(format!(
                "{remaining} steps are left and the budget allows {steps_left}"
            ));
        }
        let ran = u64::from(self.steps_run);
        if ran == 0 {
            return None;
        }
        let tokens_left = self.budget.tokens.saturating_sub(self.tokens_spent);
        let tokens_need = div_ceil(self.tokens_spent, ran).saturating_mul(remaining);
        if tokens_need > tokens_left {
            return Some(format!(
                "about {tokens_need} tokens are needed and {tokens_left} are left"
            ));
        }
        let started = self.started_ms?;
        let by_left = self.by_ms.map(|by| by.saturating_sub(now));
        let wall_left = started
            .saturating_add(self.budget.wall_ms)
            .saturating_sub(now);
        let time_left = by_left.map_or(wall_left, |by| by.min(wall_left));
        let time_need = div_ceil(now.saturating_sub(started), ran).saturating_mul(remaining);
        if time_need > time_left {
            return Some(format!(
                "about {} s are needed and {} s are left",
                div_ceil(time_need, 1000),
                time_left / 1000
            ));
        }
        None
    }

    /// The check-in due at `now`, if one is, marked as posted: call it each tick
    /// until it says `None`, since one line is posted at a time. A goal that
    /// cannot finish says so first, then the 75% line if it is behind, then a time
    /// the person set (several that passed together are one line). A finished goal
    /// and one nobody approved post none.
    pub fn due_checkin(&mut self, now: u64) -> Option<CheckIn> {
        if !self.running() {
            return None;
        }
        if !self.posted.wont_finish
            && let Some(reason) = self.cannot_finish(now)
        {
            self.posted.wont_finish = true;
            let line = format!("{}. It cannot finish: {reason}", self.progress_line());
            return Some(CheckIn {
                kind: CheckKind::WontFinish,
                line,
            });
        }
        if !self.posted.behind_checked && self.behind_mark().is_some_and(|mark| now >= mark) {
            self.posted.behind_checked = true;
            if self.is_behind(now) {
                let line = format!(
                    "{}. It is behind: {}% of the time has gone",
                    self.progress_line(),
                    self.elapsed_pct(now).unwrap_or(0)
                );
                return Some(CheckIn {
                    kind: CheckKind::Behind,
                    line,
                });
            }
        }
        let passed = self
            .checkins
            .iter()
            .skip(self.posted.scheduled as usize)
            .take_while(|&&time| time <= now)
            .count();
        if passed > 0 {
            self.posted.scheduled += passed as u32;
            return Some(CheckIn {
                kind: CheckKind::Scheduled,
                line: self.progress_line(),
            });
        }
        None
    }
}
