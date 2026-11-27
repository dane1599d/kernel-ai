//! Answers to a waiting question: a bare `y`, `n`, `show` or `why`, or
//! `answer <id> ...` from the terminal's prompt, with its ONCE and any
//! "Always allow"; `rules_remove`, and `show` of the last script.

use super::{CONFIRM_EXPIRY_NS, How, Pending};
use crate::agentcore::Showed;
use crate::script;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;
use termcore::menu::{Answer, read_answer};

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
        if let Some(reply) = self.retired_card_answer(id) {
            return reply;
        }
        match &self.pending {
            Some(pending) if Some(pending.id) == id => self.answer_pending(text),
            _ => {
                // An "Always allow" for a question no longer waiting: the node the
                // trusted path minted for it is never kept.
                if let Some((_, granted)) = read_answer(text.trim()) {
                    self.revoke_unkept(granted.rule);
                }
                Reply::err("nothing to confirm")
            }
        }
    }

    /// Answers the waiting script: `show` or `why` keeps it waiting, `y` or `yes`
    /// runs it, and anything else declines it.
    fn answer_pending(&mut self, text: &str) -> Reply {
        let Some(pending) = self.pending.take() else {
            return Reply::err("nothing to confirm");
        };
        let id = pending.id;
        // An agent's question runs only on the person's yes at its card
        // (`agentconsent`).
        let expired = self.host.now_ns().saturating_sub(pending.created_ns) > CONFIRM_EXPIRY_NS;
        if self.card_is_agents(id) && !is_show(text) && !expired {
            self.pending = Some(pending);
            let agent = match self.agent_yes_spent(id) {
                Ok(agent) => agent,
                Err(refused) => return refused,
            };
            let Some(back) = self.pending.take() else {
                return Reply::err("nothing to confirm");
            };
            // What the yes runs is the agent's: its entries say so.
            let since = self.next_id;
            let reply = self.answer_pending_checked(back, text);
            self.stamp_entries(id, since, &agent);
            return reply;
        }
        self.answer_pending_checked(pending, text)
    }

    /// [`Shell::answer_pending`] once the answer may go to `pending`.
    fn answer_pending_checked(&mut self, pending: Pending, text: &str) -> Reply {
        let id = pending.id;
        if self.host.now_ns().saturating_sub(pending.created_ns) > CONFIRM_EXPIRY_NS {
            // An "Always allow" that came too late keeps no rule (#11's sibling).
            if let Some((_, granted)) = read_answer(text.trim()) {
                self.revoke_unkept(granted.rule);
            }
            self.record(id, &pending.request, &pending.checked.script, "expired");
            self.remember(&pending, "Not run: the question expired");
            return Reply::err("the script expired; ask again");
        }
        if pending.pkg.is_some() && !is_show(text) {
            return self.answer_pkg(pending, text);
        }
        if pending.model.is_some() && !is_show(text) {
            return self.answer_model(pending, text);
        }
        if pending.app.is_some() && !is_show(text) {
            return self.answer_app(pending, text);
        }
        if pending.install.is_some() && !is_show(text) {
            return self.answer_install(pending, text);
        }
        if pending.net.is_some() && !is_show(text) {
            return self.answer_net(pending, text);
        }
        if pending.send.is_some() {
            return match is_show(text) {
                true => self.send_show(pending),
                false => self.answer_send(pending, text),
            };
        }
        if pending.standing.is_some() && !is_show(text) {
            return self.answer_instruction(pending, text);
        }
        if pending.power && !is_show(text) {
            return self.answer_power(pending, text);
        }
        if pending.goal.is_some() && !is_show(text) {
            return self.answer_goal(pending, text);
        }
        if pending.goal.is_some() && is_show(text) {
            return self.show_goal_ask(pending);
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
                self.asked_done(id, None);
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
        let unsaved = self.rule_unsaved.take();
        let lead = match (consent.strip_prefix("rule:"), unsaved) {
            (Some(rule), None) => format!(
                "Rule #{rule} added; `rules` lists it.
"
            ),
            // Kept for this boot, but never "added" as if a restart kept it.
            (Some(rule), Some(why)) => format!(
                "Rule #{rule} added for this boot: the rules were not saved ({why}), \
                 so a restart may lose it; `rules` lists it.
"
            ),
            (None, _) => String::new(),
        };
        let how = How {
            consent,
            ..pending.how
        };
        let reply = self.finish(id, &pending.request, &pending.checked, how);
        let showed = core::mem::take(&mut self.showed);
        if let Some(call) = &pending.call {
            let host = self.host.as_ref();
            crate::agentcore::remember(host, &pending.request, call, &reply.text, &showed);
        }
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
        // A stop's owner checks and spends its ONCE; checking it here first
        // would spend it before the owner could.
        if owner_checks(pending) {
            self.owner_once = Some(badge);
        } else if let Err(why) = self.host.once_check(badge, &digest) {
            self.host
                .audit(&format!("script {} ONCE refused: {why}", pending.id));
            self.revoke_unkept(granted.rule);
            return Err(format!(": its Allow did not check ({why})"));
        }
        let always = match answer {
            Answer::AlwaysKind => false,
            Answer::AlwaysTarget => true,
            _ => return Ok(String::from("yes")),
        };
        let Some(offer) = pending.offer.as_ref() else {
            self.revoke_unkept(granted.rule);
            return Err(String::from(": no rule can cover it"));
        };
        let made = (pending.id, self.host.wall_ns());
        match self.rules.add(offer, always, granted.rule, made) {
            Ok((rule, unkept)) => {
                self.host
                    .audit(&format!("rule #{rule} added from menu {}", pending.id));
                // The same rule chosen again keeps its own node.
                self.revoke_unkept(unkept);
                // A failure is audited, and the reply says the rule holds for this
                // boot only.
                self.rule_unsaved = self.save_rules().err();
                Ok(format!("rule:{rule}"))
            }
            Err(why) => {
                self.revoke_unkept(granted.rule);
                Err(format!(": {why}"))
            }
        }
    }

    /// Revokes the rule node the trusted path minted for an "Always allow"
    /// answer the shell did not keep: no rule holds its handle, so nothing
    /// else would, and it would hold a kernel rule slot until a reboot (#11).
    pub(crate) fn revoke_unkept(&self, rule: Option<u32>) {
        if let Some(Err(why)) = rule.map(|h| self.host.revoke_rule(h)) {
            self.host
                .audit(&format!("an unkept rule's node was not revoked: {why}"));
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
                match self.save_rules() {
                    Ok(()) => Reply::ok(format!(
                        "Removed rule #{id} ({}); it asks again",
                        rule.action
                    )),
                    // The removal holds now, so the reply is ok; but never
                    // "it asks again" when the disk may still hold it (#9).
                    Err(why) => Reply::ok(format!(
                        "Removed rule #{id} ({}) for this boot: the rules were not saved \
                         ({why}), so it may come back after a restart",
                        rule.action
                    )),
                }
            }
            Err(why) => Reply::err(why),
        }
    }

    /// Tells llm how a model turn that asked ended, for its conversation memory.
    pub(crate) fn remember(&self, pending: &Pending, result: &str) {
        if let Some(call) = &pending.call {
            let host = self.host.as_ref();
            crate::agentcore::remember(host, &pending.request, call, result, &Showed::default());
        }
    }

    /// `show` or `why`: the last script in full, with why it ran or asked.
    pub(crate) fn show_last(&self) -> Reply {
        let Some(text) = &self.shown else {
            return Reply::err("no script yet");
        };
        // `why` names the agent whose entry the newest one is (ADR 0013 §1).
        match self.log.entries().last().map(|e| &e.agent) {
            Some(agent) if !agent.is_kai() => Reply::ok(format!("{text}\nby {}", agent.as_str())),
            _ => Reply::ok(text.clone()),
        }
    }
}

/// Whether `pending`'s call is one whose owner checks the ONCE: a stop or a
/// restart (the kernel's `PROC_STOP_ONCE`, the supervisor's `once=`).
fn owner_checks(pending: &Pending) -> bool {
    pending
        .checked
        .plan
        .names()
        .iter()
        .any(|name| crate::rules::OWNER_CHECKED.contains(name))
}
