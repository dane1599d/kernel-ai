//! `goal_pause` and `goal_resume` (G2): each runs when the person's line
//! asks for it and asks on the card otherwise, like any other call; each is
//! logged with the state before as its undo, and neither revives a goal that
//! was cancelled or ended.

use crate::actions::How;
use crate::actlog::Change;
use crate::goalrows::Waiting;
use crate::undo::Inverse;
use crate::{Reply, Shell};
use agentabi::consent::{Decision, Why};
use alloc::format;
use alloc::string::String;
use termcore::once::{CallArg, call_text};

impl Shell {
    /// `goal_pause` (`pause`) or `goal_resume`, at the tier the door
    /// decided: one the line did not ask for, or that something read steered,
    /// asks on the card like any other call.
    pub(crate) fn goal_toggle(
        &mut self,
        id: u64,
        request: &str,
        goal: u64,
        pause: bool,
        decision: Decision,
    ) -> Reply {
        if let Err(why) = self.goal_toggles(goal) {
            return Reply::err(why);
        }
        if decision.tier == agentabi::Tier::Run {
            let consent = match decision.why {
                Why::TakenAs => "taken-as",
                _ => "asked",
            };
            return self.goal_toggle_run(id, request, goal, pause, consent);
        }
        let (row, verb) = match pause {
            true => ("goal_pause", "Pause"),
            false => ("goal_resume", "Resume"),
        };
        let call = call_text(row, &[CallArg::Int(goal as i64)]).unwrap_or_default();
        let reason = crate::agentcore::reason_of(decision.why);
        let question = format!("{verb} goal {goal} with {call}? {reason}");
        let note = format!("{verb} goal {goal}: asked first because {reason}");
        let waiting = Waiting::Toggle { goal, pause };
        self.goal_ask(id, request, &call, note, waiting, question)
    }

    /// Whether goal `goal` may be paused or resumed: one the person set that
    /// is still going.
    fn goal_toggles(&mut self, goal: u64) -> Result<(), String> {
        let kept = self.goal_book.goal(goal)?;
        match kept.state {
            "running" | "paused" => Ok(()),
            state => Err(format!("goal {goal} is {state}")),
        }
    }

    /// A pause or resume, queued, and logged with the state before as its undo.
    pub(crate) fn goal_toggle_run(
        &mut self,
        id: u64,
        request: &str,
        goal: u64,
        pause: bool,
        consent: &str,
    ) -> Reply {
        let (row, word, command) = match pause {
            true => ("goal_pause", "paused", format!("goal-pause {goal}")),
            false => ("goal_resume", "running", format!("goal-resume {goal}")),
        };
        if let Err(why) = self.goal_toggles(goal) {
            return Reply::err(why);
        }
        if let Err(why) = self.goal_book.queue(goal, command) {
            return Reply::err(why);
        }
        let before = match self.goal_book.goal(goal) {
            Ok(kept) => core::mem::replace(&mut kept.state, word),
            Err(why) => return Reply::err(why),
        };
        let call = call_text(row, &[CallArg::Int(goal as i64)]).unwrap_or_default();
        let how = How {
            form: crate::actlog::Form::Call,
            tier: agentabi::Tier::Run,
            consent: String::from(consent),
        };
        let change = Change {
            action: String::from(row),
            owner: String::from("goals"),
            res: format!("goal {goal}"),
            before: String::from(before),
            after: String::from(word),
            // Back to the state before, which may have been the same.
            inverse: Some(Inverse::Goal {
                id: goal,
                paused: before == "paused",
            }),
        };
        self.goal_logged(id, request, &call, how, change);
        Reply::ok(format!("Done: goal {goal} {word}. undo"))
    }

    /// `undo` of a pause or resume: goal `goal` paused (`paused`) or going
    /// again, while it is still going: a cancelled or ended goal stays so.
    pub(crate) fn revert_goal(&mut self, goal: u64, paused: bool) -> Result<String, String> {
        self.goal_toggles(goal)?;
        let command = match paused {
            true => format!("goal-pause {goal}"),
            false => format!("goal-resume {goal}"),
        };
        self.goal_book.queue(goal, command)?;
        let kept = self.goal_book.goal(goal)?;
        kept.state = if paused { "paused" } else { "running" };
        Ok(format!("goal {goal} {}", kept.state))
    }
}
