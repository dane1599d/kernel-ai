//! Standing instructions in the shell (P3 task 11; P3 plan's safety rules, ADR
//! 0002): `rules_add` makes one from the user's own line after an Allow on the
//! ask card; `rules` lists them beside the standing rules; `rules_remove S<n>`
//! ends one. Making and removing are log entries `undo` reverses. Each firing
//! is an entry with the rule's id, its action run by its owner through
//! [`Triggers`](crate::standing::Triggers), and undone through it too.

use crate::actions::Pending;
use crate::actlog::{Change, Entry, Form, State};
use crate::script::Checked;
use crate::standing::grammar::{self, Draft};
use crate::standing::{Event, Firing, Origin, Refused, Rule, Standing};
use crate::tiers::Tier;
use crate::undo::{Inverse, Name};
use crate::{Reply, Shell};
use agentabi::consent::Why;
use alloc::format;
use alloc::string::String;
use termcore::menu::read_answer;

impl Shell {
    /// `rules_add <rule>`: asks once on the Allow / Deny card, which shows the
    /// rule in code's words; nothing is made until the Allow.
    pub(crate) fn add_instruction(&mut self, request: &str, text: &str) -> Reply {
        let draft = match grammar::parse(text) {
            Ok(draft) => draft,
            Err(why) => return Reply::err(why),
        };
        // A typed line is the user's own (ADR 0002's injection rule).
        let checked = self.standing.check(
            Origin::Line,
            &draft.on,
            &draft.action,
            &draft.target,
            draft.until.as_ref(),
        );
        if let Err(refused) = checked {
            return Reply::err(refusal(&refused));
        }
        let id = self.next_id;
        self.next_id += 1;
        let call = format!("rules_add(\"{}\")", draft.describe());
        self.host.audit(&format!(
            "request {id}: {call} asks: it makes a standing instruction"
        ));
        let reply = self.ask_call(
            id,
            request,
            Checked::shown_only(&call),
            Why::Asked,
            None,
            None,
        );
        if let Some(pending) = &mut self.pending {
            pending.standing = Some(draft);
        }
        reply
    }

    /// The answer to a standing instruction's card: an Allow whose ONCE checks
    /// makes it.
    pub(crate) fn answer_instruction(&mut self, pending: Pending, text: &str) -> Reply {
        let id = pending.id;
        let Some(draft) = pending.standing.clone() else {
            return Reply::err("nothing to confirm");
        };
        let allowed = match (read_answer(text.trim()), &pending.menu) {
            (Some((answer, granted)), Some(call)) if answer != termcore::menu::Answer::Deny => {
                granted
                    .once
                    .ok_or(String::from("the Allow came without its ONCE"))
                    .and_then(|badge| {
                        self.host
                            .once_check(badge, &termcore::once::digest_call(id, call))
                            .map_err(|why| format!("its Allow did not check ({why})"))
                    })
            }
            _ => Err(String::new()),
        };
        if let Err(why) = allowed {
            self.record(id, &pending.request, &pending.checked.script, "declined");
            let why = match why.is_empty() {
                true => why,
                false => format!(": {why}"),
            };
            return Reply::ok(format!("No standing instruction made{why}"));
        }
        let made = self.standing.create(
            Origin::Line,
            draft.on.clone(),
            &draft.action,
            &draft.target,
            draft.until.clone(),
        );
        let rule = match made {
            Ok(rule) => rule,
            Err(refused) => return Reply::err(refusal(&refused)),
        };
        self.watch(&draft);
        self.save_standing();
        let described = draft.describe();
        self.log_instruction(
            id,
            &pending.request,
            &pending.checked.script,
            rule,
            &described,
            true,
        );
        Reply::ok(format!(
            "Standing instruction S{rule} made: {described}. undo"
        ))
    }

    /// `rules_remove S<n>`: it ends at once; what its firings did stays.
    pub(crate) fn remove_instruction(&mut self, request: &str, which: &str) -> Reply {
        let Some(rule_id) = which.strip_prefix('S').and_then(|n| n.parse::<u64>().ok()) else {
            return Reply::err("usage: rules_remove S<instruction number>");
        };
        let Some(rule) = self.standing.remove(rule_id) else {
            return Reply::err(format!("no standing instruction S{rule_id}"));
        };
        let described = rule.describe();
        self.unwatch_unused(&rule);
        self.instructions_removed.push(rule);
        self.save_standing();
        let id = self.next_id;
        self.next_id += 1;
        let script = format!("rules_remove(\"S{rule_id}\")");
        self.log_instruction(id, request, &script, rule_id, &described, false);
        Reply::ok(format!(
            "Removed standing instruction S{rule_id}; it no longer fires. undo"
        ))
    }

    /// `undo` of a `rules_add` (`made`) or a `rules_remove`; its text, or why not.
    pub(crate) fn revert_instruction(
        &mut self,
        rule_id: u64,
        made: bool,
    ) -> Result<String, String> {
        if made {
            let rule = self
                .standing
                .remove(rule_id)
                .ok_or_else(|| format!("standing instruction S{rule_id} is already gone"))?;
            self.unwatch_unused(&rule);
            self.instructions_removed.push(rule);
            self.save_standing();
            return Ok(format!("standing instruction S{rule_id} removed"));
        }
        let at = self
            .instructions_removed
            .iter()
            .rposition(|r| r.id == rule_id)
            .ok_or_else(|| {
                format!("standing instruction S{rule_id} was removed before the shell restarted")
            })?;
        let rule = self.instructions_removed.remove(at);
        let draft = draft_of(&rule);
        if let Err(refused) = self.standing.put_back(rule) {
            return Err(refusal(&refused));
        }
        self.watch(&draft);
        self.save_standing();
        Ok(format!("standing instruction S{rule_id} back"))
    }

    /// One event from the desktop or a timer: the firings it asks for run, each
    /// logged with its rule's id. A note for the panel when a rule is paused.
    pub fn on_event(&mut self, event: &Event) -> Option<String> {
        let firings = self.standing.on_event(event, self.host.now_ns());
        let mut note = None;
        for firing in firings {
            match firing {
                Firing::Run { rule, .. } => self.fire(rule, false),
                Firing::PutBack { rule } => self.fire(rule, true),
                Firing::Paused { rule } => {
                    self.host.audit(&format!(
                        "standing instruction S{rule} paused: it fired too often"
                    ));
                    self.save_standing();
                    note = Some(format!(
                        "Standing instruction S{rule} is paused: it fired more than {} times in a minute",
                        crate::standing::MAX_PER_MINUTE
                    ));
                }
            }
        }
        note
    }

    /// Runs rule `rule_id`'s action (or, `undo`, puts its last firing back),
    /// through its owner, and logs it. A firing that cannot run is refused and
    /// logged; it never asks later, when nobody is looking.
    fn fire(&mut self, rule_id: u64, undo: bool) {
        let Some(rule) = self
            .standing
            .rules()
            .iter()
            .find(|r| r.id == rule_id)
            .cloned()
        else {
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        let script = match undo {
            true => format!("undo {}", rule.call()),
            false => rule.call(),
        };
        let request = format!("S{rule_id}: {}", rule.describe());
        // The shell holds the profiles, so it switches them itself.
        if rule.action == "profile_set" {
            return self.fire_profile(id, rule_id, &rule.target, &request, undo);
        }
        let ran = match self.host.triggers() {
            Some(triggers) => triggers.run(rule_id, rule.action, &rule.target, undo),
            None => Err(String::from("no desktop events here yet")),
        };
        let (Ok(()), Some(action), Some(app)) =
            (ran.clone(), Name::new(rule.action), Name::new(&rule.target))
        else {
            let why = ran
                .err()
                .unwrap_or_else(|| String::from("its names are too long"));
            self.host
                .audit(&format!("standing instruction S{rule_id} refused: {why}"));
            self.log_firing(
                id,
                &request,
                &script,
                rule_id,
                State::Nothing,
                &format!("refused: {why}"),
                None,
            );
            return;
        };
        let change = Change {
            action: String::from(rule.action),
            owner: String::from("standing"),
            res: format!("app {}", rule.target),
            before: String::from(if undo { "changed" } else { "as it was" }),
            after: String::from(if undo { "as it was" } else { "changed" }),
            inverse: Some(Inverse::Fired {
                rule: rule_id,
                action,
                app,
                undo: !undo,
            }),
        };
        self.log_firing(
            id,
            &request,
            &script,
            rule_id,
            State::Done,
            "ran",
            Some(change),
        );
    }

    /// A standing instruction's `profile_set`: the switch, or (`undo`, its
    /// "until") the switch back, logged with the rule's id.
    fn fire_profile(&mut self, id: u64, rule: u64, name: &str, request: &str, undo: bool) {
        let logged = crate::profiles::Logged {
            form: Form::Standing,
            consent: format!("standing:{rule}"),
            request,
        };
        let done = match (
            undo,
            self.fired_profiles.iter().position(|(r, _)| *r == rule),
        ) {
            (false, _) => self.switch_profile(id, name, logged).map(|(_, key)| {
                self.fired_profiles.retain(|(r, _)| *r != rule);
                self.fired_profiles.push((rule, key));
            }),
            (true, Some(at)) => {
                let (_, key) = self.fired_profiles.remove(at);
                self.revert_entry_profile(id, (rule, key), request)
            }
            (true, None) => Err(String::from("nothing of its to put back")),
        };
        if let Err(why) = done {
            self.host
                .audit(&format!("standing instruction S{rule} refused: {why}"));
            let script = format!("profile_set(\"{name}\")");
            self.log_firing(
                id,
                request,
                &script,
                rule,
                State::Nothing,
                &format!("refused: {why}"),
                None,
            );
        }
    }

    /// An "until" putting a firing's profile switch back, as its own entry.
    fn revert_entry_profile(
        &mut self,
        id: u64,
        (rule, key): (u64, u64),
        request: &str,
    ) -> Result<(), String> {
        let (text, redo) = self.revert_profile(key)?;
        let change = Change {
            action: String::from("profile_set"),
            owner: String::from("supervisor"),
            res: String::from("profile"),
            before: String::from("changed"),
            after: text,
            inverse: Some(Inverse::Profile { key: redo }),
        };
        self.log_firing(
            id,
            request,
            "undo profile_set",
            rule,
            State::Done,
            "ran",
            Some(change),
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn log_firing(
        &mut self,
        id: u64,
        request: &str,
        script: &str,
        rule: u64,
        state: State,
        outcome: &str,
        change: Option<Change>,
    ) {
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Standing,
            of: None,
            tier: Some(Tier::Run),
            pack: None,
            consent: format!("standing:{rule}"),
            state,
            request: String::from(request),
            script: String::from(script),
            outcome: String::from(outcome),
            changes: change.into_iter().collect(),
        });
    }

    fn log_instruction(
        &mut self,
        id: u64,
        request: &str,
        script: &str,
        rule: u64,
        described: &str,
        made: bool,
    ) {
        let (before, after) = match made {
            true => (String::from("absent"), String::from(described)),
            false => (String::from(described), String::from("absent")),
        };
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: Some(if made { Tier::Ask } else { Tier::Run }),
            pack: None,
            consent: String::from(if made { "yes" } else { "asked" }),
            state: State::Done,
            request: String::from(request),
            script: String::from(script),
            outcome: String::from("ran"),
            changes: alloc::vec![Change {
                action: String::from(if made { "rules_add" } else { "rules_remove" }),
                owner: String::from("shell"),
                res: format!("standing S{rule}"),
                before,
                after,
                inverse: Some(Inverse::Instruction { id: rule, made }),
            }],
        });
    }

    /// `rules`: the standing rules, then the standing instructions.
    pub(crate) fn list_rules(&self) -> String {
        let instructions = self.standing.rules();
        let rules = self.rules.list();
        if instructions.is_empty() {
            return rules;
        }
        let mut text = match self.rules.is_empty() {
            true => String::new(),
            false => rules + "\n",
        };
        for (i, rule) in instructions.iter().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&format!("S{} {}", rule.id, rule.describe()));
        }
        text
    }

    /// The instructions `/system/state/standing` holds; a store refused whole
    /// is set aside, and the shell starts with none.
    pub(crate) fn with_saved_standing(mut self) -> Self {
        let Some(text) = self.host.standing_read() else {
            return self;
        };
        match Standing::parse(&text) {
            Some(store) => {
                self.standing = store;
                for rule in self.standing.rules().to_vec() {
                    self.watch(&draft_of(&rule));
                }
            }
            None => self
                .host
                .audit("the standing instructions on disk were refused; starting with none"),
        }
        self
    }

    fn save_standing(&self) {
        if let Err(why) = self.host.standing_write(&self.standing.text()) {
            self.host.audit(&format!(
                "the standing instructions were not written: {why}"
            ));
        }
    }

    /// Asks the event source for a rule's events; with none, the rule waits.
    fn watch(&self, draft: &Draft) {
        let Some(triggers) = self.host.triggers() else {
            return;
        };
        for when in core::iter::once(&draft.on).chain(&draft.until) {
            if let Err(why) = triggers.watch(when) {
                self.host
                    .audit(&format!("standing instruction events not watched: {why}"));
            }
        }
    }

    /// Stops the events only `rule` waited for.
    fn unwatch_unused(&self, rule: &Rule) {
        let Some(triggers) = self.host.triggers() else {
            return;
        };
        let still = self.standing.watched();
        for when in core::iter::once(&rule.on).chain(&rule.until) {
            if !still.contains(when) {
                triggers.unwatch(when);
            }
        }
    }
}

fn draft_of(rule: &Rule) -> Draft {
    Draft {
        on: rule.on.clone(),
        action: String::from(rule.action),
        target: rule.target.clone(),
        until: rule.until.clone(),
    }
}

fn refusal(refused: &Refused) -> String {
    match refused {
        Refused::FromData(source) => {
            format!("not made: it came from {source}, not your own line")
        }
        Refused::NotRunTier(action) => format!(
            "not made: {action} cannot be a standing instruction's action, which must run at once with undo"
        ),
        Refused::Bad(why) => format!("not made: {why}"),
        Refused::Full => format!(
            "not made: there are {} standing instructions already; remove one with rules_remove",
            crate::standing::MAX_RULES
        ),
    }
}
