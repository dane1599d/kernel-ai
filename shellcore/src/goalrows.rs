//! The goal rows (ADR 0013 section 5; G2): the person's standing requests,
//! carried out by the shell and run by the `goals` agent.
//!
//! - **`goal_set` asks once**, with the goal's scope (`goalscope`), budget and
//!   deadline. On the person's yes the shell sets the goal's ceiling
//!   (`agentid::Roster::grant_goal`, with the person's mark from that answer)
//!   and queues `goal-add <id>` with the goal's `kai-goals 1` file. Nothing an
//!   agent sends sets a goal or changes one: every goal row refuses an agent's
//!   step.
//! - **`goal_pause` and `goal_resume`** run, each the other's undo.
//!   **`goal_cancel`** asks, then drops the ceiling at once and queues the
//!   cancel. Its steps stay done; `undo goals` undoes them.
//! - **The queue.** Routes form no cycle (ADR 0001), so the shell never calls
//!   `goals`: `goals` takes each queued command with `agent-step next` (`ok
//!   #<n> <command>`, or `ok none`) and answers it with `agent-step report <n>
//!   <result>`. Only `goals`'s badge may take the queue; it holds at most
//!   [`MAX_QUEUED`].

use crate::actions::{How, Pending};
use crate::actlog::{Change, Entry, State};
use crate::script::Checked;
use crate::{Reply, Shell};
use agentabi::consent::Decision;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use termcore::menu::{Menu, Origin};
use termcore::once::{CallArg, call_text};

/// The most commands waiting for `goals`, taken or not.
pub const MAX_QUEUED: usize = 16;
/// Kai's badge: the shell's own caller, which sets a goal's ceiling.
const KAI_BADGE: u32 = 1;
/// What an agent's step to a goal row is told.
pub const PERSON_ONLY: &str = "goals are the person's: an agent may not set or change one";
/// What every agent but `goals` is told at the queue.
pub const GOALS_ONLY: &str = "only the goals agent takes the goal queue";

/// What a goal row's ask waits on.
#[derive(Clone, Debug)]
pub(crate) enum Waiting {
    Set(goalcore::Spec),
    Cancel(u64),
    Toggle { goal: u64, pause: bool },
}

/// A goal the person set, as the shell knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Kept {
    id: u64,
    text: String,
    pub(crate) state: &'static str,
    /// What `goals` last reported for it.
    pub(crate) last: String,
}

/// A command for `goals`, and whether it was taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Queued {
    pub(crate) n: u64,
    pub(crate) goal: u64,
    pub(crate) command: String,
    pub(crate) taken: bool,
}

/// The goals the person set and the queue to `goals`.
#[derive(Clone, Debug, Default)]
pub(crate) struct GoalBook {
    goals: Vec<Kept>,
    pub(crate) queue: Vec<Queued>,
    next_n: u64,
    next_goal: u64,
}

impl GoalBook {
    pub(crate) fn queue(&mut self, goal: u64, command: String) -> Result<(), String> {
        if self.queue.len() >= MAX_QUEUED {
            return Err(String::from(
                "the goals agent has not taken its last changes yet; try again soon",
            ));
        }
        self.next_n += 1;
        self.queue.push(Queued {
            n: self.next_n,
            goal,
            command,
            taken: false,
        });
        Ok(())
    }

    /// Queues goal `goal`'s cancel whatever the queue holds: its commands
    /// not taken yet go, so the cancel never waits for room.
    fn queue_cancel(&mut self, goal: u64) {
        let unadded = self
            .queue
            .iter()
            .any(|q| q.goal == goal && !q.taken && q.command.starts_with("goal-add "));
        self.queue.retain(|q| q.goal != goal || q.taken);
        // A goal `goals` never took has nothing to cancel there.
        if unadded {
            return;
        }
        self.next_n += 1;
        self.queue.push(Queued {
            n: self.next_n,
            goal,
            command: format!("goal-cancel {goal}"),
            taken: false,
        });
    }

    /// Room for one more goal at `goals`, which keeps at most
    /// [`goalcore::MAX_GOALS`] unfinished: checked before the person is asked,
    /// and again before the yes grants a ceiling, so `goal-add` is not refused
    /// after it (bug-hunt-6 H6-25). A goal here running or paused may still be
    /// unfinished there; every other state has ended.
    fn room(&self) -> Result<(), String> {
        let unfinished = self
            .goals
            .iter()
            .filter(|g| matches!(g.state, "running" | "paused"))
            .count();
        match unfinished < goalcore::MAX_GOALS {
            true => Ok(()),
            false => Err(format!(
                "{} goals are running or paused; cancel one first",
                goalcore::MAX_GOALS
            )),
        }
    }

    pub(crate) fn goal(&mut self, id: u64) -> Result<&mut Kept, String> {
        self.goals
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or_else(|| format!("no goal {id}; goal_list shows them"))
    }

    fn list(&self) -> String {
        if self.goals.is_empty() {
            return String::from("no goals");
        }
        let lines: Vec<String> = self
            .goals
            .iter()
            .map(|g| match g.last.is_empty() {
                true => format!("goal {} {}: {}", g.id, g.state, g.text),
                false => format!("goal {} {}: {} ({})", g.id, g.state, g.text, g.last),
            })
            .collect();
        lines.join("\n")
    }
}

/// `name`'s argument `arg`, as text.
fn value<'a>(values: &'a [(&'static str, String)], arg: &str) -> Option<&'a str> {
    values
        .iter()
        .find(|(n, _)| *n == arg)
        .map(|(_, v)| v.as_str())
}

impl Shell {
    /// A goal row's call, which `call_row` hands here with its tier decided.
    pub(crate) fn goal_call(
        &mut self,
        id: u64,
        request: &str,
        row: &str,
        values: &[(&'static str, String)],
        decision: Decision,
    ) -> Reply {
        if self.agent_acting().is_some() {
            return Reply::err(PERSON_ONLY);
        }
        let goal = value(values, "id").and_then(|v| v.parse::<u64>().ok());
        match (row, goal) {
            ("goal_list", _) => Reply::ok(self.goal_book.list()),
            ("goal_set", _) => self.goal_set(id, request, values),
            ("goal_pause", Some(goal)) => self.goal_toggle(id, request, goal, true, decision),
            ("goal_resume", Some(goal)) => self.goal_toggle(id, request, goal, false, decision),
            ("goal_cancel", Some(goal)) => self.goal_cancel(id, request, goal),
            _ => Reply::err(format!("{row} needs its id")),
        }
    }

    /// `goal_set`: the spec checked, then the one ask with its approval.
    fn goal_set(&mut self, id: u64, request: &str, values: &[(&'static str, String)]) -> Reply {
        if let Err(why) = self.goal_book.room() {
            return Reply::err(format!("no goal set: {why}"));
        }
        // The scope is the person's line's, never the model's arguments'.
        let spec = match self.goal_spec(request, values) {
            Ok(spec) => spec,
            Err(why) => return Reply::err(format!("no goal set: {why}")),
        };
        // The card holds 120 characters of code-written words: the counts of
        // what the goal may use and spend; `show` lists every row and folder.
        // The spec the yes sets waits in the pending ask, so the short call
        // binds the same goal the ONCE names, and `grant_goal` gets exactly
        // the scope `show` listed.
        let call = call_text("goal_set", &[CallArg::Int(id as i64)]).unwrap_or_default();
        let question = card_counts(&spec, &call);
        let note = spec.approval();
        self.goal_ask(id, request, &call, note, Waiting::Set(spec), question)
    }

    /// The goal `values` describe, its scope from the person's `line`, held
    /// to goalcore's limits.
    fn goal_spec(
        &self,
        line: &str,
        values: &[(&'static str, String)],
    ) -> Result<goalcore::Spec, String> {
        let text = value(values, "text").unwrap_or_default().trim().to_string();
        let number = |arg: &str| value(values, arg).and_then(|v| v.parse::<u64>().ok());
        let now = self.host.now_ns() / 1_000_000;
        let by_ms = match value(values, "by") {
            // Checked: a duration that fits a u64 may not fit one added to now.
            Some(by) => Some(
                now.checked_add(crate::goalscope::duration_ms(by)?)
                    .ok_or_else(|| format!("{by:?}: write a time as 90m, 2h or 1d"))?,
            ),
            None => None,
        };
        let budget = goalcore::Budget {
            steps: number("steps")
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            tokens: number("tokens").unwrap_or(0),
            wall_ms: crate::goalscope::duration_ms(value(values, "wall").unwrap_or_default())?,
        };
        let spec = goalcore::Spec {
            scope: crate::goalscope::scope(line, |folder| {
                self.host.fs_read(crate::FsRead::List(folder)).map(|_| ())
            })?,
            text,
            by_ms,
            budget,
            checkins: Vec::new(),
            away_ok: true,
        };
        spec.check(now)?;
        Ok(spec)
    }

    /// `goal_cancel`: asks with what it stops.
    fn goal_cancel(&mut self, id: u64, request: &str, goal: u64) -> Reply {
        let text = match self.goal_book.goal(goal) {
            Ok(kept) if kept.state == "cancelled" => {
                return Reply::err(format!("goal {goal} was cancelled"));
            }
            Ok(kept) => kept.text.clone(),
            Err(why) => return Reply::err(why),
        };
        let call = call_text("goal_cancel", &[CallArg::Int(goal as i64)]).unwrap_or_default();
        let question = format!(
            "Cancel goal {goal} for good with {call}? Its steps stay done; type show for its words"
        );
        let note = format!(
            "goal {goal}, \"{text}\", stops for good; its steps stay done (undo goals undoes them)"
        );
        self.goal_ask(id, request, &call, note, Waiting::Cancel(goal), question)
    }

    /// Asks once for a goal row on a card whose question is `question`,
    /// code-written, with Deny highlighted; `note` says in full what it does
    /// on `show`. Only the card's Allow, with its ONCE, carries it out, so a
    /// question too long for the card is refused, never asked as y/n.
    pub(crate) fn goal_ask(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        note: String,
        waiting: Waiting,
        question: String,
    ) -> Reply {
        let menu = Menu::new(id, &question, Some(""), Some(""), Origin::Added)
            .and_then(|menu| menu.with_call(call));
        if menu.is_err() {
            return Reply::err(format!("not asked: {call} does not fit its card"));
        }
        self.host.audit(&format!("request {id}: {call} asks"));
        let mut checked = Checked::shown_only(call);
        checked.note = Some(note);
        self.shown = Some(format!(
            "{}
asked first: a goal row always asks",
            crate::script::shown(id, &checked)
        ));
        self.pending = Some(Pending {
            id,
            request: request.to_string(),
            checked,
            asking: Vec::new(),
            created_ns: self.host.now_ns(),
            how: How {
                form: crate::actlog::Form::Call,
                tier: agentabi::Tier::Ask,
                consent: String::from("asked"),
            },
            call: None,
            offer: None,
            menu: Some(String::from(call)),
            pkg: None,
            standing: None,
            model: None,
            app: None,
            install: None,
            net: None,
            send: None,
            power: false,
            goal: Some(waiting),
        });
        Reply::menu(id, Menu::encode(id, Origin::Added, &question, "", "", call))
    }

    /// `show` for a goal row's ask: what it sets, in full, and that only its
    /// card's Allow or Deny answers it (a typed y does not).
    pub(crate) fn show_goal_ask(&mut self, pending: Pending) -> Reply {
        let id = pending.id;
        let shown = crate::script::shown(id, &pending.checked);
        self.pending = Some(pending);
        Reply {
            ok: true,
            text: format!(
                "goal ask {id}: answer it on its card, Allow or Deny
{shown}"
            ),
            confirm: Some(id),
            more: false,
        }
    }

    /// The answer to a goal row's ask: an Allow whose ONCE checks carries it
    /// out, under the person's mark.
    pub(crate) fn answer_goal(&mut self, pending: Pending, text: &str) -> Reply {
        let Some(waiting) = pending.goal.clone() else {
            return Reply::err("nothing to confirm");
        };
        if let Err(reply) = self.allowed(&pending, text) {
            return reply;
        }
        let how = How {
            consent: String::from("yes"),
            ..pending.how.clone()
        };
        let reply = match waiting {
            Waiting::Set(spec) => self.goal_made(&pending, spec, how),
            Waiting::Cancel(goal) => self.goal_cancelled(&pending, goal, how),
            Waiting::Toggle { goal, pause } => {
                self.goal_toggle_run(pending.id, &pending.request, goal, pause, "yes")
            }
        };
        self.remember(&pending, &reply.text);
        reply
    }

    /// The person's yes to `goal_set`: the ceiling, then the goal queued.
    fn goal_made(&mut self, pending: &Pending, spec: goalcore::Spec, how: How) -> Reply {
        // Goal ids are never used twice, across restarts too: the log keeps
        // every goal this shell set.
        let goal = self.goal_book.next_goal.max(self.last_logged_goal() + 1);
        // Goals set while this card was open may have taken the last place.
        if let Err(why) = self.goal_book.room() {
            return Reply::err(format!("no goal set: {why}"));
        }
        let Ok(kai) = self.roster.caller(KAI_BADGE) else {
            return Reply::err("no goal set: the shell is not on its roster");
        };
        let mark = Self::console_mark();
        let granted = self
            .roster
            .grant_goal(&kai, mark, goal, &spec.scope.rows, &agentabi::ROWS);
        if let Err(why) = granted {
            return Reply::err(format!("no goal set: {why}"));
        }
        // The folders the card counted, with the ceiling: one value.
        self.roster.keep_goal_folders(goal, &spec.scope.folders);
        let now = self.host.now_ns() / 1_000_000;
        let file = match goal_file(goal, spec.clone(), now) {
            Ok(file) => file,
            Err(why) => {
                self.roster.revoke_goal(goal);
                return Reply::err(format!("no goal set: {why}"));
            }
        };
        if let Err(why) = self
            .goal_book
            .queue(goal, format!("goal-add {goal}\n{file}"))
        {
            self.roster.revoke_goal(goal);
            return Reply::err(format!("no goal set: {why}"));
        }
        self.goal_book.next_goal = goal + 1;
        self.goal_book.goals.push(Kept {
            id: goal,
            text: spec.text.clone(),
            state: "running",
            last: String::new(),
        });
        let change = Change {
            action: String::from("goal_set"),
            owner: String::from("goals"),
            res: format!("goal {goal}"),
            before: String::new(),
            after: String::from("running"),
            inverse: None,
        };
        // The log keeps the goal's own words, not the card's short call.
        let call = call_text("goal_set", &[CallArg::Text(&spec.text)]).unwrap_or_default();
        self.goal_logged(pending.id, &pending.request, &call, how, change);
        Reply::ok(format!(
            "Goal {goal} set; the goals agent works on it. goal_list shows how it goes"
        ))
    }

    /// The person's yes to `goal_cancel`: the ceiling goes at once.
    fn goal_cancelled(&mut self, pending: &Pending, goal: u64, how: How) -> Reply {
        // The ceiling goes first, whatever else happens: the goal can take no
        // step from the yes on.
        self.roster.revoke_goal(goal);
        let before = match self.goal_book.goal(goal) {
            Ok(kept) => core::mem::replace(&mut kept.state, "cancelled"),
            Err(why) => return Reply::err(why),
        };
        self.goal_book.queue_cancel(goal);
        let change = Change {
            action: String::from("goal_cancel"),
            owner: String::from("goals"),
            res: format!("goal {goal}"),
            before: String::from(before),
            after: String::from("cancelled"),
            inverse: None,
        };
        let call = pending.checked.script.clone();
        self.goal_logged(pending.id, &pending.request, &call, how, change);
        Reply::ok(format!("Goal {goal} cancelled"))
    }

    /// The highest goal id the log has a change for, or 0.
    fn last_logged_goal(&self) -> u64 {
        self.log
            .entries()
            .flat_map(|entry| entry.changes.iter())
            .filter(|change| change.action == "goal_set")
            .filter_map(|change| {
                // Read back from disk, `goal 1` is `goal_1` (bug-hunt-3 H3-24).
                let n = change.res.strip_prefix("goal")?.strip_prefix([' ', '_'])?;
                n.parse::<u64>().ok()
            })
            .max()
            .unwrap_or(0)
    }

    /// What `goal_list` shows.
    #[cfg(test)]
    pub(crate) fn goal_book_list(&self) -> String {
        self.goal_book.list()
    }

    pub(crate) fn goal_logged(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        how: How,
        change: Change,
    ) {
        self.log.push(Entry {
            agent: agentid::Name::kai(),
            id,
            t_ns: self.host.now_ns(),
            form: how.form,
            of: None,
            tier: Some(how.tier),
            pack: None,
            consent: how.consent,
            state: State::Done,
            request: String::from(request),
            script: String::from(call),
            outcome: String::from("ran"),
            changes: alloc::vec![change],
        });
        self.save_log();
    }
}

/// What `goal_set`'s card says, in code-written words only: how many actions
/// and folders, and the most it may spend.
fn card_counts(spec: &goalcore::Spec, call: &str) -> String {
    let count = |n: usize, one: &str, many: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    format!(
        "Goal: {} in {}, up to {} steps and {} tokens, with {call}? Type show to see them",
        count(spec.scope.rows.len(), "action", "actions"),
        count(spec.scope.folders.len(), "folder", "folders"),
        spec.budget.steps,
        spec.budget.tokens,
    )
}

/// The `kai-goals 1` file of the one goal `goal`, approved at `now`.
fn goal_file(goal: u64, spec: goalcore::Spec, now: u64) -> Result<String, String> {
    let header = format!("{}\nids\t{goal}\n", goalcore::MAGIC);
    let mut goals = goalcore::Goals::parse(&header)?;
    let made = goals.set(spec, now).map_err(|why| why.words())?;
    goals.approve(made, now).map_err(|why| why.words())?;
    Ok(goals.to_text())
}
