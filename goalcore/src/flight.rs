//! The step that is in flight, and how it comes out: it finished (ran or failed),
//! the person said no on the card, or the card lapsed unanswered.

use crate::goal::{Goal, Pause, State, StepStatus};
use crate::run::{Refusal, StepDone};
use crate::valid;
use alloc::format;
use alloc::string::String;

/// The note on a step the goal's end cut off while it ran: failed as far as the
/// goal knows, until its late finish says how it came out.
pub const ENDED_IN_FLIGHT: &str = "the goal ended while this step ran";
/// The note on such a step once its late finish came.
pub const FINISHED_AFTER_END: &str = "it finished after the goal ended";

/// The note on a step whose question was left unanswered.
pub const LAPSED: &str = "its question was not answered in time";
/// What a goal plans next once the same step has lapsed twice in a row.
pub const LAPSED_NEXT: &str = "not answered in time; resume to ask again";

impl Goal {
    /// The running step ended: what it cost, the entry it made, and whether the
    /// door answered ok. It counts against the budget either way.
    ///
    /// A step the goal's end cut off while it ran ([`ENDED_IN_FLIGHT`]) may still
    /// finish once, after the end: its entry, its tokens and how it came out are
    /// kept, so `undo goals` and cancel's offer find what it did (the door made the
    /// entry whether or not the goal was still going).
    pub fn finish_step(&mut self, done: StepDone) -> Result<(), Refusal> {
        let Some(n) = self.flight.take() else {
            return self.finish_after_end(done);
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
                // The door's words never read as the end's own mark.
                let why = match valid::clean(&why) {
                    why if why == ENDED_IN_FLIGHT => format!("failed: {why}"),
                    why => why,
                };
                (StepStatus::Failed, Some(why))
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

    /// The late finish of the step the goal's end cut off: its last step, failed
    /// with [`ENDED_IN_FLIGHT`] and no entry yet. Once only: the note changes, so a
    /// second report finds no step.
    fn finish_after_end(&mut self, done: StepDone) -> Result<(), Refusal> {
        let ended = self.state.is_final();
        let Some(step) = self.log.last_mut().filter(|step| {
            ended
                && step.status == StepStatus::Failed
                && step.entry.is_none()
                && step.note == ENDED_IN_FLIGHT
        }) else {
            return Err(Refusal::NoStepRunning);
        };
        step.entry = done.entry;
        step.tokens = done.tokens;
        match done.failed {
            None => {
                step.status = StepStatus::Ran;
                step.note = String::from(FINISHED_AFTER_END);
                self.failed -= 1;
                self.steps_ok += 1;
            }
            Some(why) => {
                step.note = valid::clean(&format!("{FINISHED_AFTER_END}, failed: {why}"));
            }
        }
        self.tokens_spent = self.tokens_spent.saturating_add(done.tokens);
        Ok(())
    }

    /// The running step was not run: the person said no on the card, where the one
    /// door asked for it (the goals agent's one ask, after the step left goalcore as
    /// one that runs). It is counted as asked and denied, not as run, as
    /// [`Goal::deny`] counts a step goalcore itself held for a yes.
    pub fn deny_running(&mut self, why: &str) -> Result<(), Refusal> {
        let Some(n) = self.flight.take() else {
            return Err(Refusal::NoStepRunning);
        };
        self.asked = self.asked.saturating_add(1);
        self.denied = self.denied.saturating_add(1);
        if let Some(step) = self.log.iter_mut().rev().find(|s| s.n == n) {
            step.status = StepStatus::Denied;
            step.note = valid::clean(why);
        }
        Ok(())
    }

    /// The running step's card lapsed unanswered: no no, so it is not counted
    /// as denied and is not remembered as refused; it is held, and the goal
    /// goes on, free to plan it again.
    pub fn lapse_running(&mut self) -> Result<(), Refusal> {
        let Some(n) = self.flight.take() else {
            return Err(Refusal::NoStepRunning);
        };
        self.asked = self.asked.saturating_add(1);
        if let Some(step) = self.log.iter_mut().rev().find(|s| s.n == n) {
            step.status = StepStatus::Held;
            step.note = String::from(LAPSED);
        }
        Ok(())
    }

    /// The same step lapsed twice in a row, so the goal stops asking: it pauses
    /// as the person's own pause does, and their resume lets it be asked once
    /// more. The step stays held and out of the refused ones.
    pub fn pause_lapsed(&mut self) -> Result<(), Refusal> {
        if self.state != State::Active {
            return Err(Refusal::NotActive(self.state));
        }
        let planned = self.planned;
        self.plan(planned, LAPSED_NEXT);
        self.state = State::Paused(Pause::User);
        Ok(())
    }
}
