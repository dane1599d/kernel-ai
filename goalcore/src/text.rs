//! `kai-goals 1`: the goals as text, for the file the `goals` agent keeps. One
//! field to a tab (drawn here as `|`), one line to a record, in a fixed order:
//!
//! ```text
//! kai-goals 1
//! ids|3
//! goal|2|waiting
//! text|draft the quarterly report
//! by|1800000000000
//! away|1
//! made|1790000000000
//! started|1790000001000
//! ended|-
//! note|
//! rows|fs_list|fs_read|notes_*
//! folders|reports
//! budget|8|60000|3600000
//! checkins|1795000000000
//! plan|4|write the summary
//! spent|1|1|0|0|1200
//! posted|0|0|0
//! waiting|tier|fs_trash has no undo, so it asks|fs_trash|reports/old.txt|ask|200
//! step|1|1790000002000|ran|41|1200|run|900|fs_list|reports
//! step|2|1790000009000|asked|-|0|ask|200|fs_trash|reports/old.txt|fs_trash has no undo, so it asks
//! end
//! ```
//!
//! The text is read as hostile, as a file that comes back from disk: any line out
//! of shape, and any record that does not add up (a count that is not the log's, a
//! waiting step that is not the last), refuses the whole file with its line. What
//! reads writes back byte for byte.

use crate::goal::{
    Ask, AskKind, Budget, Goal, Posted, Proposal, Scope, State, StepLog, StepStatus, Tier,
};
use crate::goals::Goals;
use crate::valid;
use crate::{
    MAX_ACTIVE, MAX_CHECKINS, MAX_FOLDERS, MAX_GOALS, MAX_LOG, MAX_ROWS, MAX_STEPS, MAX_TEXT,
    MAX_TOKENS, MAX_WALL_MS,
};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

/// The first line of the file.
pub const MAGIC: &str = "kai-goals 1";
/// The largest file read: 16 goals of 128 steps fit well inside it.
const MAX_FILE: usize = 512 * 1024;

fn tier_word(tier: Tier) -> &'static str {
    match tier {
        Tier::Run => "run",
        Tier::Ask => "ask",
    }
}

fn kind_word(kind: AskKind) -> &'static str {
    match kind {
        AskKind::Tier => "tier",
        AskKind::Scope => "scope",
        AskKind::Away => "away",
    }
}

fn number(value: Option<u64>) -> String {
    value.map_or_else(|| String::from("-"), |n| n.to_string())
}

fn target(path: &Option<String>) -> &str {
    path.as_deref().unwrap_or("-")
}

impl Goals {
    /// The file's text.
    pub fn to_text(&self) -> String {
        let mut out = format!("{MAGIC}\nids\t{}\n", self.next_id);
        for goal in &self.list {
            write_goal(&mut out, goal);
        }
        out
    }

    /// The goals a file holds, or why it is refused, with its line.
    pub fn parse(text: &str) -> Result<Self, String> {
        Reader::new(text)?.goals()
    }
}

fn write_goal(out: &mut String, g: &Goal) {
    let _ = writeln!(out, "goal\t{}\t{}", g.id, g.state.word());
    let _ = writeln!(out, "text\t{}", g.text);
    let _ = writeln!(out, "by\t{}", number(g.by_ms));
    let _ = writeln!(out, "away\t{}", u8::from(g.away_ok));
    let _ = writeln!(out, "made\t{}", g.made_ms);
    let _ = writeln!(out, "started\t{}", number(g.started_ms));
    let _ = writeln!(out, "ended\t{}", number(g.ended_ms));
    let _ = writeln!(out, "note\t{}", g.note);
    for (key, items) in [("rows", &g.scope.rows), ("folders", &g.scope.folders)] {
        out.push_str(key);
        for item in items {
            out.push('\t');
            out.push_str(item);
        }
        out.push('\n');
    }
    let b = &g.budget;
    let _ = writeln!(out, "budget\t{}\t{}\t{}", b.steps, b.tokens, b.wall_ms);
    out.push_str("checkins");
    for time in &g.checkins {
        let _ = write!(out, "\t{time}");
    }
    out.push('\n');
    let _ = writeln!(out, "plan\t{}\t{}", g.planned, g.next);
    let _ = writeln!(
        out,
        "spent\t{}\t{}\t{}\t{}\t{}",
        g.steps_run, g.steps_ok, g.failed, g.denied, g.tokens_spent
    );
    let p = &g.posted;
    let _ = writeln!(
        out,
        "posted\t{}\t{}\t{}",
        p.scheduled,
        u8::from(p.behind_checked),
        u8::from(p.wont_finish)
    );
    if let Some(ask) = &g.waiting {
        let p = &ask.proposal;
        let _ = writeln!(
            out,
            "waiting\t{}\t{}\t{}\t{}\t{}\t{}",
            kind_word(ask.kind),
            ask.why,
            p.row,
            target(&p.target),
            tier_word(p.tier),
            p.tokens_est
        );
    }
    if let Some(n) = g.flight {
        let _ = writeln!(out, "flight\t{n}");
    }
    for s in &g.log {
        let _ = writeln!(
            out,
            "step\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            s.n,
            s.at_ms,
            s.status.word(),
            number(s.entry),
            s.tokens,
            tier_word(s.proposal.tier),
            s.proposal.tokens_est,
            s.proposal.row,
            target(&s.proposal.target),
            s.note
        );
    }
    out.push_str("end\n");
}

/// A line cursor that says which line was wrong.
struct Reader<'a> {
    lines: Vec<&'a str>,
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str) -> Result<Self, String> {
        if text.len() > MAX_FILE {
            return Err(String::from("the file is too large"));
        }
        if text.contains('\r') {
            return Err(String::from("the file has a carriage return"));
        }
        let Some(body) = text.strip_suffix('\n') else {
            return Err(String::from("the last line does not end with a newline"));
        };
        let lines: Vec<&str> = body.split('\n').collect();
        if lines.first() != Some(&MAGIC) {
            return Err(format!("line 1: not a {MAGIC} file"));
        }
        Ok(Self { lines, at: 1 })
    }

    fn bad<T>(&self, why: &str) -> Result<T, String> {
        Err(format!("line {}: {why}", self.at + 1))
    }

    fn done(&self) -> bool {
        self.at >= self.lines.len()
    }

    fn key(&self) -> Option<&'a str> {
        self.lines.get(self.at)?.split('\t').next()
    }

    /// The fields after `key` on the next line, which must have `min` to `max`.
    fn take(&mut self, key: &str, min: usize, max: usize) -> Result<Vec<&'a str>, String> {
        let Some(line) = self.lines.get(self.at) else {
            return Err(format!(
                "line {}: the file ends before `{key}`",
                self.at + 1
            ));
        };
        let mut fields = line.split('\t');
        if fields.next() != Some(key) {
            return self.bad(&format!("expected `{key}`"));
        }
        let fields: Vec<&str> = fields.collect();
        if fields.len() < min || fields.len() > max {
            return self.bad(&format!("`{key}` has the wrong number of fields"));
        }
        self.at += 1;
        Ok(fields)
    }

    /// One field of `key`'s line.
    fn one(&mut self, key: &str) -> Result<&'a str, String> {
        Ok(self.take(key, 1, 1)?[0])
    }

    fn num(&self, field: &str) -> Result<u64, String> {
        let canonical = !field.is_empty()
            && field.bytes().all(|b| b.is_ascii_digit())
            && (field == "0" || !field.starts_with('0'));
        match (canonical, field.parse::<u64>()) {
            (true, Ok(n)) => Ok(n),
            _ => Err(format!("line {}: `{field}` is not a number", self.at)),
        }
    }

    fn num32(&self, field: &str) -> Result<u32, String> {
        u32::try_from(self.num(field)?)
            .map_err(|_| format!("line {}: `{field}` is too large", self.at))
    }

    fn flag(&self, field: &str) -> Result<bool, String> {
        match field {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(format!("line {}: `{field}` is not 0 or 1", self.at)),
        }
    }

    fn maybe(&self, field: &str) -> Result<Option<u64>, String> {
        match field {
            "-" => Ok(None),
            _ => self.num(field).map(Some),
        }
    }

    fn words(&self, field: &str, empty: bool) -> Result<String, String> {
        let ok = match empty {
            true => valid::note_ok(field),
            false => valid::words_ok(field, MAX_TEXT),
        };
        match ok {
            true => Ok(String::from(field)),
            false => Err(format!("line {}: those words are not allowed", self.at)),
        }
    }

    fn path(&self, field: &str) -> Result<Option<String>, String> {
        match field {
            "-" => Ok(None),
            _ if valid::printable(field) && !field.is_empty() && field.len() <= 255 => {
                Ok(Some(String::from(field)))
            }
            _ => Err(format!("line {}: that is not a path", self.at)),
        }
    }

    fn tier(&self, field: &str) -> Result<Tier, String> {
        match field {
            "run" => Ok(Tier::Run),
            "ask" => Ok(Tier::Ask),
            _ => Err(format!("line {}: `{field}` is not a tier", self.at)),
        }
    }

    fn proposal(&self, row: &str, path: &str, tier: &str, est: u64) -> Result<Proposal, String> {
        if !valid::row_name_ok(row) {
            return Err(format!("line {}: `{row}` is not a row", self.at));
        }
        Ok(Proposal {
            row: String::from(row),
            target: self.path(path)?,
            tier: self.tier(tier)?,
            tokens_est: est,
        })
    }

    fn goals(&mut self) -> Result<Goals, String> {
        let ids = self.one("ids")?;
        let next_id = self.num(ids)?;
        let mut list: Vec<Goal> = Vec::new();
        while !self.done() {
            if self.key() != Some("goal") {
                return self.bad("expected `goal`");
            }
            if list.len() == MAX_GOALS {
                return self.bad("there are too many goals");
            }
            let goal = self.goal()?;
            let ascending = list.last().is_none_or(|last| last.id < goal.id);
            if goal.id == 0 || !ascending || goal.id >= next_id {
                return Err(format!(
                    "line {}: goal numbers are not 1 or more and ascending, below `ids`",
                    self.at
                ));
            }
            list.push(goal);
        }
        if list.iter().filter(|goal| goal.state.is_live()).count() > MAX_ACTIVE {
            return Err(format!("more than {MAX_ACTIVE} goals are active"));
        }
        Ok(Goals { list, next_id })
    }

    fn goal(&mut self) -> Result<Goal, String> {
        let head = self.take("goal", 2, 2)?;
        let id = self.num(head[0])?;
        let state = State::from_word(head[1]);
        let Some(state) = state else {
            return self.bad("not a state");
        };
        let text = self.one("text")?;
        let text = self.words(text, false)?;
        let by_ms = self.one("by")?;
        let by_ms = self.maybe(by_ms)?;
        let away = self.one("away")?;
        let away_ok = self.flag(away)?;
        let made = self.one("made")?;
        let made_ms = self.num(made)?;
        let started = self.one("started")?;
        let started_ms = self.maybe(started)?;
        let ended = self.one("ended")?;
        let ended_ms = self.maybe(ended)?;
        let note = self.one("note")?;
        let note = self.words(note, true)?;
        let rows = self.take("rows", 1, MAX_ROWS)?;
        let rows: Vec<String> = rows.iter().map(|r| String::from(*r)).collect();
        let folders = self.take("folders", 0, MAX_FOLDERS)?;
        let folders: Vec<String> = folders.iter().map(|f| String::from(*f)).collect();
        let budget = self.take("budget", 3, 3)?;
        let budget = Budget {
            steps: self.num32(budget[0])?,
            tokens: self.num(budget[1])?,
            wall_ms: self.num(budget[2])?,
        };
        let times = self.take("checkins", 0, MAX_CHECKINS)?;
        let checkins: Vec<u64> = times
            .iter()
            .map(|t| self.num(t))
            .collect::<Result<_, _>>()?;
        let plan = self.take("plan", 2, 2)?;
        let planned = self.num32(plan[0])?;
        let next = self.words(plan[1], true)?;
        let spent = self.take("spent", 5, 5)?;
        let counts = [
            self.num32(spent[0])?,
            self.num32(spent[1])?,
            self.num32(spent[2])?,
            self.num32(spent[3])?,
        ];
        let tokens_spent = self.num(spent[4])?;
        let posted = self.take("posted", 3, 3)?;
        let posted = Posted {
            scheduled: self.num32(posted[0])?,
            behind_checked: self.flag(posted[1])?,
            wont_finish: self.flag(posted[2])?,
        };
        let waiting = match self.key() {
            Some("waiting") => Some(self.waiting()?),
            _ => None,
        };
        let flight = match self.key() {
            Some("flight") => {
                let n = self.one("flight")?;
                Some(self.num32(n)?)
            }
            _ => None,
        };
        let mut log = Vec::new();
        while self.key() == Some("step") {
            if log.len() == MAX_LOG {
                return self.bad("the log is too long");
            }
            log.push(self.step(log.len() as u32 + 1)?);
        }
        self.take("end", 0, 0)?;
        let goal = Goal {
            id,
            text,
            by_ms,
            scope: Scope { rows, folders },
            budget,
            checkins,
            away_ok,
            state,
            made_ms,
            started_ms,
            ended_ms,
            planned,
            next,
            note,
            steps_run: counts[0],
            steps_ok: counts[1],
            failed: counts[2],
            denied: counts[3],
            tokens_spent,
            waiting,
            flight,
            posted,
            log,
        };
        self.consistent(&goal)?;
        Ok(goal)
    }

    fn waiting(&mut self) -> Result<Ask, String> {
        let f = self.take("waiting", 6, 6)?;
        let kind = match f[0] {
            "tier" => AskKind::Tier,
            "scope" => AskKind::Scope,
            "away" => AskKind::Away,
            _ => return Err(format!("line {}: not a kind of ask", self.at)),
        };
        let why = self.words(f[1], false)?;
        let est = self.num(f[5])?;
        Ok(Ask {
            kind,
            why,
            proposal: self.proposal(f[2], f[3], f[4], est)?,
        })
    }

    fn step(&mut self, n: u32) -> Result<StepLog, String> {
        let f = self.take("step", 10, 10)?;
        if self.num32(f[0])? != n {
            return Err(format!("line {}: steps are not numbered from 1", self.at));
        }
        let Some(status) = StepStatus::from_word(f[2]) else {
            return Err(format!("line {}: not a step status", self.at));
        };
        let est = self.num(f[6])?;
        Ok(StepLog {
            n,
            at_ms: self.num(f[1])?,
            proposal: self.proposal(f[7], f[8], f[5], est)?,
            status,
            entry: self.maybe(f[3])?,
            tokens: self.num(f[4])?,
            note: self.words(f[9], true)?,
        })
    }

    /// Whether a goal that parsed line by line adds up: its limits, its counts and
    /// its waiting and running steps agree with its state and its log.
    fn consistent(&self, g: &Goal) -> Result<(), String> {
        let line = self.at;
        let fail = |why: &str| Err(format!("line {line}: goal #{} {why}", g.id));
        let scope_ok = g.scope.rows.iter().all(|row| valid::scope_row_ok(row))
            && g.scope.folders.iter().all(|folder| valid::path_ok(folder));
        let distinct = |items: &[String]| {
            items
                .iter()
                .enumerate()
                .all(|(at, item)| !items[..at].contains(item))
        };
        if !scope_ok || !distinct(&g.scope.rows) || !distinct(&g.scope.folders) {
            return fail("has a scope that is not allowed");
        }
        let b = &g.budget;
        if b.steps == 0 || b.steps > MAX_STEPS || b.tokens == 0 || b.tokens > MAX_TOKENS {
            return fail("has a budget out of range");
        }
        if b.wall_ms == 0 || b.wall_ms > MAX_WALL_MS || g.planned > MAX_STEPS {
            return fail("has a budget or a plan out of range");
        }
        let ascending = g.checkins.windows(2).all(|pair| pair[0] < pair[1]);
        if !ascending || g.posted.scheduled as usize > g.checkins.len() {
            return fail("has check-ins that are not in order");
        }
        // A goal declined before it started was never started.
        let started = g.started_ms.is_some();
        let may_be_unstarted = matches!(g.state, State::Proposed | State::Cancelled);
        if (g.state == State::Proposed && started) || (!may_be_unstarted && !started) {
            return fail("has a start that its state does not allow");
        }
        if g.ended_ms.is_some() != g.state.is_final() {
            return fail("has an end that its state does not allow");
        }
        let count = |status: StepStatus| g.log.iter().filter(|s| s.status == status).count() as u32;
        let (ran, failed) = (count(StepStatus::Ran), count(StepStatus::Failed));
        let (denied, running) = (count(StepStatus::Denied), count(StepStatus::Running));
        let asked = count(StepStatus::Asked);
        if g.steps_run != ran + failed || g.steps_ok != ran || g.failed != failed {
            return fail("has step counts that are not its log's");
        }
        if g.denied != denied || g.steps_run > g.budget.steps {
            return fail("has denials that are not its log's, or ran past its budget");
        }
        let spent: u64 = g.log.iter().map(|s| s.tokens).sum();
        if g.tokens_spent < spent {
            return fail("has spent fewer tokens than its steps did");
        }
        // The waiting step and the running one are the last, and match the state.
        let last = g.log.last();
        match (&g.waiting, g.flight, g.state) {
            (Some(ask), None, State::Waiting) => {
                let matches = last.is_some_and(|s| {
                    s.status == StepStatus::Asked
                        && s.proposal.row == ask.proposal.row
                        && s.proposal.target == ask.proposal.target
                        && s.proposal.tier == ask.proposal.tier
                });
                if !matches || asked != 1 || running != 0 {
                    return fail("waits on a step that is not its last");
                }
            }
            (None, flight, state) if state != State::Waiting => {
                let wanted = flight.map(|n| n as usize);
                let at_last = last.map(|s| (s.n as usize, s.status == StepStatus::Running));
                let ok = match (wanted, at_last) {
                    (None, _) => running == 0,
                    (Some(n), Some((last_n, true))) => n == last_n && running == 1,
                    _ => false,
                };
                if !ok || asked != 0 {
                    return fail("has a running or asked step its state does not match");
                }
            }
            _ => return fail("has a waiting step or a running one its state does not match"),
        }
        Ok(())
    }
}
