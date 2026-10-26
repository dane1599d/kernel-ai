//! What happens to a checked script: a run-tier one runs at once and the reply
//! says what happened; one that asks gets one short question, answered by `y` or
//! `n` (or `answer <id> ...` from the terminal's prompt). `undo` puts back the last
//! run's changes, and `show` or `why` shows the last script in full.
//!
//! Only lines the shell is given count as requests, and the shell takes lines
//! from the kernel terminal alone (see the `shell` program), so text a script
//! reads never starts an action.

use crate::actlog::{Entry, Form, State};
use crate::rules::Offer;
use crate::script::{self, Checked};
use crate::tiers::{self, Tier};
use crate::undo::{self, Done};
use crate::{Reply, Shell};
use agentabi::consent::{Decision, Why};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use termcore::menu::{Answer, read_answer};

/// How long a question waits for its answer.
const CONFIRM_EXPIRY_NS: u64 = 120_000_000_000;

/// How a run came to be, for its log entry.
#[derive(Clone, Debug)]
pub(crate) struct How {
    pub form: Form,
    pub tier: Tier,
    /// `asked`, `taken-as`, `yes`, `rule:<id>`, `-`.
    pub consent: String,
}

/// A script waiting for the user's answer.
pub(crate) struct Pending {
    pub id: u64,
    pub request: String,
    pub checked: Checked,
    pub asking: Vec<&'static str>,
    pub created_ns: u64,
    pub how: How,
    /// The model's call, when a tagged call asked, for llm's conversation memory.
    pub call: Option<String>,
    /// The standing rules its menu offers, when a rule may waive its ask.
    pub offer: Option<Offer>,
    /// The call its Allow / Deny menu showed, canonical, which the Allow's
    /// `ONCE` digest must name; `None` for a y/n question.
    pub menu: Option<String>,
    /// For a package switch, the switch that waits on the answer.
    pub pkg: Option<crate::pkg::Switch>,
}

/// `y`, `n`, `yes` or `no`, any case.
fn yes_or_no(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(true),
        "n" | "no" => Some(false),
        _ => None,
    }
}

/// Each call of `checked`'s plan with its consent for `request`.
fn decided(request: &str, checked: &Checked) -> Vec<(&'static str, Decision)> {
    let decisions = checked.plan.decide(request, &agentabi::ROWS);
    checked
        .plan
        .sites
        .iter()
        .map(|site| site.name)
        .zip(decisions)
        .collect()
}

fn is_show(text: &str) -> bool {
    matches!(text.trim(), "show" | "why")
}

impl Shell {
    /// The tier of a checked script written for `request`: each call's consent from
    /// the line and the labels of its values (ADR 0002 D3, D6), raised to its row's
    /// tier in the table, and the calls at that tier.
    pub(crate) fn tier_of(&self, request: &str, checked: &Checked) -> (Tier, Vec<&'static str>) {
        let tiered: Vec<(&'static str, Tier)> = decided(request, checked)
            .into_iter()
            .map(|(name, decision)| (name, decision.tier.max(tiers::of(self.tiers, name))))
            .collect();
        let tier = tiered
            .iter()
            .map(|&(_, tier)| tier)
            .max()
            .unwrap_or(Tier::Run);
        let mut at = Vec::new();
        for &(name, at_tier) in &tiered {
            if at_tier == tier && !at.contains(&name) {
                at.push(name);
            }
        }
        (tier, at)
    }

    /// The question for a script that asks about `asking`: the content that steers
    /// a call, or added by the model if every one of them is run-tier in the table,
    /// or else for having no undo.
    fn question(&self, id: u64, request: &str, checked: &Checked, asking: &[&str]) -> String {
        let steered =
            decided(request, checked)
                .into_iter()
                .find_map(|(_, decision)| match decision.why {
                    Why::Steered(source) | Why::Outbound(source) => Some(source),
                    _ => None,
                });
        let added = asking
            .iter()
            .all(|name| tiers::of(self.tiers, name) == Tier::Run);
        let why = match (steered, added) {
            (Some(source), _) => format!(" (the target came from {source})"),
            (None, true) => String::from(", which the request did not ask for"),
            (None, false) => String::from(" (no undo)"),
        };
        script::question(id, checked, asking, &why)
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
            let taken_as = matches!(
                decided(request, &checked).as_slice(),
                [(
                    _,
                    Decision {
                        why: Why::TakenAs,
                        ..
                    }
                )]
            );
            let how = How {
                form: Form::Script,
                tier,
                consent: String::from(if taken_as { "taken-as" } else { "asked" }),
            };
            return self.finish(id, request, &checked, how);
        }
        self.host
            .audit(&format!("script {id} tier ask ({}); asking", at.join(", ")));
        let text = self.question(id, request, &checked, &at);
        self.pending = Some(Pending {
            id,
            request: request.to_string(),
            checked,
            asking: at,
            created_ns: self.host.now_ns(),
            how: How {
                form: Form::Script,
                tier,
                consent: String::from("asked"),
            },
            call: None,
            offer: None,
            menu: None,
            pkg: None,
        });
        Reply {
            ok: true,
            text,
            confirm: Some(id),
            more: false,
        }
    }

    /// Runs a script the user asked for, logs what it changed, and says what
    /// happened in a line or two after its output: `Done: policy round-robin (was
    /// interactive). undo`, or `Taken as: …` when it ran a vague line's reading. A
    /// run that fails after a change puts the change back: `Nothing changed: …`.
    pub(crate) fn finish(&mut self, id: u64, request: &str, checked: &Checked, how: How) -> Reply {
        let ran = script::run(checked, &self.host, &self.config);
        for done in &ran.done {
            let tier = tiers::of(self.tiers, &done.action);
            self.host.audit(&format!(
                "script {id} {} ({}): {}",
                done.action,
                tier.name(),
                done.what()
            ));
        }
        self.last_output = Some(ran.text.clone());
        if !ran.ok && !ran.done.is_empty() {
            return self.roll_back(id, request, checked, how, ran);
        }
        let outcome = match (ran.ok, checked.changes.is_empty()) {
            (true, true) => "ran read-only",
            (false, true) => "failed read-only",
            (true, false) => "ran",
            (false, false) => "failed",
        };
        let undoable = !ran.done.is_empty() && ran.done.iter().all(|d| d.inverse.is_some());
        let mut text = ran.text;
        if !ran.done.is_empty() {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            let what: Vec<String> = ran.done.iter().map(Done::what).collect();
            let lead = if how.consent == "taken-as" {
                "Taken as"
            } else {
                "Done"
            };
            let tail = if undoable { ". undo" } else { "" };
            text.push_str(&format!("{lead}: {}{tail}", what.join("; ")));
        } else if text.is_empty() && ran.ok {
            text = match checked.summary.is_empty() {
                true => String::from("Done."),
                false => format!("Done: {}", checked.summary),
            };
        }
        let state = match ran.ok {
            true => State::Done,
            false => State::Nothing,
        };
        self.log_entry(id, request, checked, how, state, outcome, ran.done);
        Reply {
            ok: ran.ok,
            text,
            confirm: None,
            more: false,
        }
    }

    /// A run that failed after changing something: its changes go back, newest
    /// first, so the script was all or nothing.
    fn roll_back(
        &mut self,
        id: u64,
        request: &str,
        checked: &Checked,
        how: How,
        ran: script::Ran,
    ) -> Reply {
        let mut left = ran.done.clone();
        while let Some(change) = left.last() {
            let put_back = change
                .inverse
                .is_some_and(|inverse| undo::revert(self.host.as_ref(), inverse, false).is_ok());
            if !put_back {
                break;
            }
            left.pop();
        }
        let error = ran.text.lines().last().unwrap_or("it failed").to_string();
        if left.is_empty() {
            self.host.audit(&format!("script {id} failed; rolled back"));
            self.log_entry(
                id,
                request,
                checked,
                how,
                State::Nothing,
                "failed; nothing changed",
                Vec::new(),
            );
            // The output before the error, then the error once, as what changed nothing.
            let output = ran
                .text
                .trim_end()
                .rsplit_once('\n')
                .map_or("", |(before, _)| before);
            let text = match output.is_empty() {
                true => format!("Nothing changed: {error}"),
                false => format!("{output}\nNothing changed: {error}"),
            };
            return Reply::err(text);
        }
        let what: Vec<String> = left.iter().map(Done::what).collect();
        self.log_entry(id, request, checked, how, State::Done, "failed", left);
        Reply::err(format!(
            "{}\nChanged before the error, and could not be put back: {}. undo",
            ran.text.trim_end(),
            what.join("; ")
        ))
    }

    /// Logs a run with its changes.
    #[allow(clippy::too_many_arguments)]
    fn log_entry(
        &mut self,
        id: u64,
        request: &str,
        checked: &Checked,
        how: How,
        state: State,
        outcome: &str,
        changes: Vec<Done>,
    ) {
        self.host.audit(&format!("script {id} {outcome}"));
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: how.form,
            of: None,
            tier: Some(how.tier),
            consent: how.consent.clone(),
            state,
            request: String::from(request),
            script: checked.script.clone(),
            outcome: String::from(outcome),
            changes,
        });
    }

    /// Logs a request that ran nothing: refused, declined, answered in words.
    pub(crate) fn record(&mut self, id: u64, request: &str, script: &str, outcome: &str) {
        self.host.audit(&format!("script {id} {outcome}"));
        let form = match script.is_empty() {
            true => Form::Words,
            false => Form::Script,
        };
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form,
            of: None,
            tier: None,
            consent: String::from("-"),
            state: State::Nothing,
            request: String::from(request),
            script: String::from(script),
            outcome: String::from(outcome),
            changes: Vec::new(),
        });
    }

    /// A bare `y`, `n`, `yes`, `no`, `show` or `why` while a question waits answers
    /// it. `None` when the line is something else, or names a variable.
    pub(crate) fn answer_bare(&mut self, input: &str) -> Option<Reply> {
        let yes = yes_or_no(input);
        if (yes.is_none() && !is_show(input)) || self.scope.contains(input) {
            return None;
        }
        if yes.is_some() && self.tuner.proposing() {
            let reply = self.tuner.answer(self.host.as_ref(), input);
            self.log_kept_trial();
            return Some(reply);
        }
        if let Some(yes) = yes
            && let Some(id) = self.undo_asking.take()
        {
            return Some(self.undo_answer(id, yes));
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
            let reply = self.tuner.answer(self.host.as_ref(), text);
            self.log_kept_trial();
            return reply;
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
            self.remember(&pending, "Not run: the question expired");
            return Reply::err("the script expired; ask again");
        }
        if pending.pkg.is_some() && !is_show(text) {
            return self.answer_pkg(pending, text);
        }
        if is_show(text) {
            let question = self.question(id, &pending.request, &pending.checked, &pending.asking);
            let shown = script::shown(id, &pending.checked);
            self.pending = Some(pending);
            return Reply {
                ok: true,
                text: format!("{shown}\n{question}"),
                confirm: Some(id),
                more: false,
            };
        }
        let consent = match self.consent_from(&pending, text) {
            Ok(consent) => consent,
            Err(note) => {
                self.record(id, &pending.request, &pending.checked.script, "declined");
                self.remember(&pending, "Not run: declined");
                return Reply::ok(format!("script {id} not run{note}"));
            }
        };
        match consent.as_str() {
            "yes" => self
                .host
                .audit(&format!("script {id} confirmed; running it")),
            rule => self
                .host
                .audit(&format!("script {id} confirmed, {rule}; running it")),
        }
        let lead = match consent.strip_prefix("rule:") {
            Some(rule) => format!(
                "Rule #{rule} added; `rules` lists it.
"
            ),
            None => String::new(),
        };
        let how = How {
            consent,
            ..pending.how
        };
        let reply = self.finish(id, &pending.request, &pending.checked, how);
        self.remember(&pending, &reply.text);
        Reply {
            text: format!("{lead}{}", reply.text),
            ..reply
        }
    }

    /// The consent an answer gives: `y`/`yes` or the menu's Allow give `yes`; an
    /// "Always allow" choice writes the rule its menu offered and gives
    /// `rule:<id>`. Anything else, or a rule that was not offered, declines, with a
    /// note on why when there is one.
    ///
    /// A menu's yes must come with the `ONCE` the trusted path minted for it, and
    /// that `ONCE` must check, through the kernel, against the question exactly as
    /// shown; a match uses it up. A typed `y` cannot answer a menu.
    fn consent_from(&mut self, pending: &Pending, text: &str) -> Result<String, String> {
        let Some((answer, granted)) = read_answer(text.trim()) else {
            return match (yes_or_no(text), &pending.menu) {
                (Some(true), None) => Ok(String::from("yes")),
                (Some(true), Some(_)) => Err(String::from(": a menu is answered on the menu")),
                _ => Err(String::new()),
            };
        };
        if answer == Answer::Deny {
            return Err(String::new());
        }
        let Some(call) = &pending.menu else {
            return Err(String::from(": that question was not a menu"));
        };
        let badge = granted.once.ok_or(": the Allow came without its ONCE")?;
        let digest = termcore::once::digest_call(pending.id, call);
        if let Err(why) = self.host.once_check(badge, &digest) {
            self.host
                .audit(&format!("script {} ONCE refused: {why}", pending.id));
            return Err(format!(": its Allow did not check ({why})"));
        }
        let always = match answer {
            Answer::AlwaysKind => false,
            Answer::AlwaysTarget => true,
            _ => return Ok(String::from("yes")),
        };
        let offer = pending.offer.as_ref().ok_or(": no rule can cover it")?;
        match self.rules.add(offer, always, granted.rule) {
            Ok(rule) => {
                self.host
                    .audit(&format!("rule #{rule} added from menu {}", pending.id));
                Ok(format!("rule:{rule}"))
            }
            Err(why) => Err(format!(": {why}")),
        }
    }

    /// `rules_remove <n>`: the rule goes at once; what ran under it stays.
    pub(crate) fn remove_rule(&mut self, which: &str) -> Reply {
        let Ok(id) = which.trim().trim_start_matches('#').parse::<u64>() else {
            return Reply::err("usage: rules_remove <rule number>");
        };
        match self.rules.remove(id) {
            Ok(rule) => {
                // Its grant node goes too; nothing waits on it.
                if let Some(Err(why)) = rule.grant.map(|h| self.host.revoke_rule(h)) {
                    self.host
                        .audit(&format!("rule #{id}: its node was not revoked: {why}"));
                }
                self.host.audit(&format!("rule #{id} removed"));
                Reply::ok(format!(
                    "Removed rule #{id} ({}); it asks again",
                    rule.action
                ))
            }
            Err(why) => Reply::err(why),
        }
    }

    /// Tells llm how a model turn that asked ended, for its conversation memory.
    pub(crate) fn remember(&self, pending: &Pending, result: &str) {
        if let Some(call) = &pending.call {
            crate::agentcore::remember(self.host.as_ref(), &pending.request, call, result);
        }
    }

    /// `show` or `why`: the last script in full, with why it ran or asked.
    pub(crate) fn show_last(&self) -> Reply {
        match &self.shown {
            Some(text) => Reply::ok(text.clone()),
            None => Reply::err("no script yet"),
        }
    }
}
