//! What happens to a checked script: a run-tier one runs at once and the reply
//! says what happened; one that asks gets one short question, answered by `y` or
//! `n` (or `answer <id> ...` from the terminal's prompt). `undo` puts back the last
//! run's changes, and `show` or `why` shows the last script in full.
//!
//! Only lines the shell is given count as requests, and the shell takes lines
//! from the kernel terminal alone (see the `shell` program), so text a script
//! reads never starts an action.

use crate::script::{self, Checked};
use crate::tiers::{self, Tier};
use crate::undo::{self, Done, Refused};
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// How long a question waits for its answer.
const CONFIRM_EXPIRY_NS: u64 = 120_000_000_000;

/// A script waiting for the user's answer.
pub(crate) struct Pending {
    pub id: u64,
    pub request: String,
    pub checked: Checked,
    pub asking: Vec<&'static str>,
    pub created_ns: u64,
}

/// The last run's changes, for `undo`.
pub(crate) struct Undoable {
    pub id: u64,
    pub done: Vec<Done>,
}

/// `y`, `n`, `yes` or `no`, any case.
fn yes_or_no(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

fn is_show(text: &str) -> bool {
    matches!(text.trim(), "show" | "why")
}

impl Shell {
    /// The tier of a checked script written for `request`, from every function it
    /// names, and the functions at that tier.
    pub(crate) fn tier_of(&self, request: &str, checked: &Checked) -> (Tier, Vec<&'static str>) {
        let names: Vec<&'static str> = checked
            .reads
            .iter()
            .chain(&checked.changes)
            .copied()
            .collect();
        tiers::of_script(self.tiers, request, &names)
    }

    /// The question for a script that asks about `asking`: added by the model if
    /// every one of them is run-tier in the table, or else for having no undo.
    fn question(&self, id: u64, checked: &Checked, asking: &[&str]) -> String {
        let added = asking
            .iter()
            .all(|name| tiers::of(self.tiers, name) == Tier::Run);
        script::question(id, checked, asking, added)
    }

    /// Runs a run-tier script at once, or asks about one that is not.
    pub(crate) fn propose(
        &mut self,
        id: u64,
        request: &str,
        checked: Checked,
        (tier, at): (Tier, Vec<&'static str>),
    ) -> Reply {
        let why = match (tier, checked.changes.is_empty()) {
            (Tier::Run, true) => String::from("ran at once: it only reads"),
            (Tier::Run, false) => String::from("ran at once: its changes can be undone"),
            _ if at
                .iter()
                .all(|name| tiers::of(self.tiers, name) == Tier::Run) =>
            {
                format!("asked first: the request did not ask for {}", at.join(", "))
            }
            _ => format!("asked first: {} has no undo", at.join(", ")),
        };
        self.shown = Some(format!("{}\n{why}", script::shown(id, &checked)));
        if tier == Tier::Run {
            self.host
                .audit(&format!("script {id} tier run; running it"));
            return self.finish(id, request, &checked, false);
        }
        self.host
            .audit(&format!("script {id} tier ask ({}); asking", at.join(", ")));
        let text = self.question(id, &checked, &at);
        self.pending = Some(Pending {
            id,
            request: request.to_string(),
            checked,
            asking: at,
            created_ns: self.host.now_ns(),
        });
        Reply {
            ok: true,
            text,
            confirm: Some(id),
            more: false,
        }
    }

    /// Runs a script the user asked for, records what it changed, and says what
    /// happened in a line or two after its output: `Done: policy round-robin (was
    /// interactive). undo`, or `Taken as: …` when it ran a vague line's reading.
    pub(crate) fn finish(
        &mut self,
        id: u64,
        request: &str,
        checked: &Checked,
        taken_as: bool,
    ) -> Reply {
        let ran = script::run(checked, &self.host, &self.config);
        for done in &ran.done {
            let tier = tiers::of(self.tiers, done.function);
            self.host.audit(&format!(
                "script {id} {} ({}): {}",
                done.function,
                tier.name(),
                done.what
            ));
        }
        let undoable = !ran.done.is_empty() && ran.done.iter().all(|d| d.inverse.is_some());
        if !ran.done.is_empty() {
            // `undo` is for the last action only, so one with no undo clears it.
            self.undo = undoable.then(|| Undoable {
                id,
                done: ran.done.clone(),
            });
        }
        let outcome = match (ran.ok, checked.changes.is_empty()) {
            (true, true) => "ran read-only",
            (false, true) => "failed read-only",
            (true, false) => "ran",
            (false, false) => "failed",
        };
        self.record(id, request, &checked.script, outcome);
        self.last_output = Some(ran.text.clone());
        let mut text = ran.text;
        if !ran.done.is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            let what: Vec<&str> = ran.done.iter().map(|d| d.what.as_str()).collect();
            let lead = match (ran.ok, taken_as) {
                (true, true) => "Taken as",
                (true, false) => "Done",
                (false, _) => "Changed before the error",
            };
            let tail = if undoable { ". undo" } else { "" };
            text.push_str(&format!("{lead}: {}{tail}", what.join("; ")));
        } else if text.is_empty() && ran.ok {
            text = match checked.summary.is_empty() {
                true => String::from("Done."),
                false => format!("Done: {}", checked.summary),
            };
        }
        Reply {
            ok: ran.ok,
            text,
            confirm: None,
            more: false,
        }
    }

    pub(crate) fn record(&mut self, id: u64, request: &str, script: &str, outcome: &str) {
        self.log.record(id, request, script, outcome);
        self.host.audit(&format!("script {id} {outcome}"));
    }

    /// A bare `y`, `n`, `yes`, `no`, `show` or `why` while a question waits answers
    /// it. `None` when the line is something else, or names a variable.
    pub(crate) fn answer_bare(&mut self, input: &str) -> Option<Reply> {
        let yes = yes_or_no(input);
        if (yes.is_none() && !is_show(input)) || self.scope.contains(input) {
            return None;
        }
        if yes.is_some() && self.tuner.proposing() {
            return Some(self.tuner.answer(self.host.as_ref(), input));
        }
        if let Some(yes) = yes
            && let Some(undoable) = self.undo_asking.take()
        {
            self.undo = Some(undoable);
            return Some(match yes {
                true => self.undo_with(true),
                false => Reply::ok("Not undone; `undo` asks again"),
            });
        }
        if self.pending.is_some() {
            return Some(self.answer_pending(input));
        }
        // `show` and `why` with nothing waiting show the last script.
        yes.map(|_| Reply::err("nothing to answer"))
    }

    /// `answer <id> <text>` from the terminal's prompt, for a tune proposal or a
    /// script waiting under `id`.
    pub(crate) fn answer(&mut self, rest: &str) -> Reply {
        let (id, text) = rest.split_once(' ').unwrap_or((rest, ""));
        let id = id.parse::<u64>().ok();
        if id.is_some_and(|id| self.tuner.waiting(id)) {
            return self.tuner.answer(self.host.as_ref(), text);
        }
        match &self.pending {
            Some(pending) if Some(pending.id) == id => self.answer_pending(text),
            _ => Reply::err("nothing to confirm"),
        }
    }

    /// Answers the waiting script: `show` or `why` keeps it waiting, `y` or `yes`
    /// runs it, and anything else declines it.
    fn answer_pending(&mut self, text: &str) -> Reply {
        let Some(pending) = self.pending.take() else {
            return Reply::err("nothing to confirm");
        };
        let id = pending.id;
        if self.host.now_ns().saturating_sub(pending.created_ns) > CONFIRM_EXPIRY_NS {
            self.record(id, &pending.request, &pending.checked.script, "expired");
            return Reply::err("the script expired; ask again");
        }
        if is_show(text) {
            let question = self.question(id, &pending.checked, &pending.asking);
            let shown = script::shown(id, &pending.checked);
            self.pending = Some(pending);
            return Reply {
                ok: true,
                text: format!("{shown}\n{question}"),
                confirm: Some(id),
                more: false,
            };
        }
        if yes_or_no(text) != Some(true) {
            self.record(id, &pending.request, &pending.checked.script, "declined");
            return Reply::ok(format!("script {id} not run"));
        }
        self.host
            .audit(&format!("script {id} confirmed; running it"));
        self.finish(id, &pending.request, &pending.checked, false)
    }

    /// `undo`: puts back what the last run changed, newest change first. A change
    /// that cannot be put back yet stays for another `undo`.
    pub(crate) fn undo(&mut self) -> Reply {
        self.undo_with(false)
    }

    /// `undo`, and a yes to its question: a change that something else moved since
    /// is put back only `anyway`; otherwise the user is asked, with `y` or `n`.
    fn undo_with(&mut self, anyway: bool) -> Reply {
        let Some(mut undoable) = self.undo.take() else {
            return Reply::err("nothing to undo");
        };
        let id = undoable.id;
        let mut said = Vec::new();
        while let Some(done) = undoable.done.pop() {
            let Some(inverse) = done.inverse else {
                continue;
            };
            match undo::revert(self.host.as_ref(), inverse, anyway) {
                Ok(text) => said.push(text),
                Err(Refused::Moved(now)) => {
                    undoable.done.push(done);
                    self.undo_asking = Some(undoable);
                    self.host
                        .audit(&format!("script {id} undo asks: changed since (now {now})"));
                    let mut text = match said.is_empty() {
                        true => String::new(),
                        false => format!("Undone: {}\n", said.join("; ")),
                    };
                    text.push_str(&format!(
                        "The policy changed since (now {now}); put {} back anyway? y/n",
                        undo::target(inverse)
                    ));
                    return Reply::ok(text);
                }
                Err(Refused::Failed(why)) => {
                    undoable.done.push(done);
                    self.undo = Some(undoable);
                    self.host.audit(&format!("script {id} undo failed: {why}"));
                    return Reply::err(format!("{why}; try undo again"));
                }
            }
        }
        self.host.audit(&format!("script {id} undone"));
        Reply::ok(format!("Undone: {}", said.join("; ")))
    }

    /// `show` or `why`: the last script in full, with why it ran or asked.
    pub(crate) fn show_last(&self) -> Reply {
        match &self.shown {
            Some(text) => Reply::ok(text.clone()),
            None => Reply::err("no script yet"),
        }
    }
}
