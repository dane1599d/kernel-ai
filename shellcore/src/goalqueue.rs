//! The queue from the shell to `goals` (G2): routes form no cycle (ADR
//! 0001), so the shell never calls `goals`. `goals` takes each queued command
//! with `agent-step next` (`ok #<n> <command>`, or `ok none`) and answers it
//! with `agent-step report <n> <result>`. Only `goals`'s badge may take the
//! queue; the goal rows (`goalrows`) fill it.

use crate::goalrows::GOALS_ONLY;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;

impl Shell {
    /// `agent-step next` and `agent-step report <n> <result>`, for `goals`'s
    /// badge only: `None` for any other step.
    pub(crate) fn goal_queue_step(
        &mut self,
        caller: &agentid::Caller,
        text: &str,
    ) -> Option<Reply> {
        let report = text.strip_prefix("report ");
        if text != "next" && report.is_none() {
            return None;
        }
        if caller.name().as_str() != "goals" {
            return Some(Reply::err(GOALS_ONLY));
        }
        Some(match report {
            Some(rest) => self.goal_report(rest),
            None => self.goal_next(),
        })
    }

    /// The oldest command not taken yet, now taken.
    fn goal_next(&mut self) -> Reply {
        match self.goal_book.queue.iter_mut().find(|q| !q.taken) {
            Some(queued) => {
                queued.taken = true;
                Reply::ok(format!("#{} {}", queued.n, queued.command))
            }
            None => Reply::ok("none"),
        }
    }

    /// What came of taken command `<n>`: kept for `goal_list`, and its place
    /// freed. A goal `goals` refused to add, or reports ended (done, failed,
    /// cancelled, expired) or no longer has (forgotten), loses its ceiling.
    fn goal_report(&mut self, rest: &str) -> Reply {
        let (n, result) = rest.split_once(' ').unwrap_or((rest, ""));
        let Ok(n) = n.trim_start_matches('#').parse::<u64>() else {
            return Reply::err("usage: agent-step report <n> <result>");
        };
        let at = self
            .goal_book
            .queue
            .iter()
            .position(|q| q.n == n && q.taken);
        let Some(at) = at else {
            return Reply::err(format!("no taken command #{n}"));
        };
        let queued = self.goal_book.queue.remove(at);
        // A goal-list report names each goal and its state, a line each.
        // goals sends it as `ok <id> <state> ...`, a line a goal.
        for line in result.strip_prefix("ok ").unwrap_or(result).lines() {
            let mut words = line.split_whitespace();
            let (Some(goal), Some(word)) = (words.next(), words.next()) else {
                continue;
            };
            if let (Ok(goal), Some(word)) = (goal.parse::<u64>(), ended(word)) {
                self.goal_ended(goal, word);
            }
        }
        let no_goal: alloc::vec::Vec<&str> = result.split_whitespace().take(4).collect();
        if no_goal == ["err", "no", "goal", format!("{}", queued.goal).as_str()] {
            self.goal_ended(queued.goal, "forgotten");
        }
        let result = one_line(result);
        self.host
            .audit(&format!("goals reports #{n} ({}): {result}", queued.goal));
        let refused_add = queued.command.starts_with("goal-add ") && result.starts_with("err");
        if refused_add {
            self.roster.revoke_goal(queued.goal);
        }
        if let Ok(kept) = self.goal_book.goal(queued.goal) {
            if refused_add {
                kept.state = "not started";
            }
            kept.last = result;
        }
        Reply::ok(format!("reported #{n}"))
    }

    /// Goal `goal` takes no more steps (`word` says why): its ceiling goes.
    fn goal_ended(&mut self, goal: u64, word: &'static str) {
        self.roster.revoke_goal(goal);
        if let Ok(kept) = self.goal_book.goal(goal) {
            kept.state = word;
        }
    }
}

/// The words goals reports a goal that takes no more steps with (goalcore's
/// final states, and an expired one).
const ENDED: [&str; 4] = ["done", "failed", "cancelled", "expired"];

/// `word` if it is one of [`ENDED`].
fn ended(word: &str) -> Option<&'static str> {
    ENDED.iter().copied().find(|ended| *ended == word)
}

/// What goals reported, as one printable line for `goal_list` and the audit:
/// every character but printable ASCII (a newline, a control) a space, runs of spaces one, at
/// most `goalcore::MAX_TEXT` characters.
fn one_line(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        let c = if (' '..='~').contains(&c) { c } else { ' ' };
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        if out.chars().count() >= goalcore::MAX_TEXT {
            break;
        }
        out.push(c);
    }
    String::from(out.trim_end())
}
