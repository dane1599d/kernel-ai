//! Installs where P1 touches them (ADR 0011 §9-§10), over `pkg`'s store: typed
//! package lines and the `pkg_*` rows the AI calls. Each switch is a log entry
//! whose change is the generation switch, so `undo` puts the old set back,
//! compare-and-set on `current`.
//!
//! Consent (ADR 0011 §10): an install the user's own line names runs, unless it
//! needs a permission ADR 0001 makes the user choose, which asks once; one the AI
//! picks asks once, as consent's `Added` decides. A remove or rollback runs.

use crate::actions::{How, Pending};
use crate::actlog::{Change, Entry, Form, State};
use crate::pkg::Switch;
use crate::pkg::line::{Command, Line};
use crate::script::Checked;
use crate::tiers::Tier;
use crate::undo::{Inverse, Refused};
use crate::{Reply, Shell};
use agentabi::consent::{Decision, Why};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use termcore::menu::read_answer;

/// Said once per shell when a line starts with `sudo`.
const NO_SUDO: &str = "no sudo needed: the question is the permission";

impl Shell {
    /// A typed package line. Reads answer at once; a switch runs, or asks when
    /// it needs a new permission.
    pub(crate) fn pkg_line(&mut self, request: &str, parsed: Result<Line, String>) -> Reply {
        let line = match parsed {
            Ok(line) => line,
            Err(why) => return Reply::err(why),
        };
        let reply = self.pkg_command(request, line.command, String::from("typed"));
        match line.sudo && !self.said_no_sudo {
            true => {
                self.said_no_sudo = true;
                Reply {
                    text: format!("{NO_SUDO}\n{}", reply.text),
                    ..reply
                }
            }
            false => reply,
        }
    }

    fn pkg_command(&mut self, request: &str, command: Command, consent: String) -> Reply {
        let store = &self.pkgs;
        let (call, switch) = match command {
            Command::List => return Reply::ok(store.list()),
            Command::History => return Reply::ok(store.history()),
            Command::Info(name) => return Reply::ok(store.info(&name)),
            Command::Search(text) => return Reply::ok(store.search(&text)),
            Command::Upgrade(_) => {
                return Reply::ok(
                    "nothing to upgrade: the image's repository has one version of each",
                );
            }
            Command::Install(names) => (call("pkg_install", &names), store.install(&names)),
            Command::Remove(names) => (call("pkg_remove", &names), store.remove(&names)),
            Command::Rollback(n) => (
                format!(
                    "pkg_rollback({})",
                    n.map_or(String::new(), |n| n.to_string())
                ),
                store.rollback(n),
            ),
        };
        let switch = match switch {
            Ok(switch) => switch,
            Err(why) => return Reply::err(why),
        };
        let id = self.next_id;
        self.next_id += 1;
        let how = How {
            form: Form::Call,
            tier: Tier::Run,
            consent,
        };
        match switch.asks() {
            false => self.pkg_switch(id, request, &call, switch, how),
            true => self.pkg_ask(id, request, &call, switch, Why::NoUndo),
        }
    }

    /// A `pkg_*` row the AI called, after consent: a read answers, a switch runs
    /// when the line named it and it needs no new permission, and asks otherwise.
    pub(crate) fn pkg_call(
        &mut self,
        id: u64,
        request: &str,
        row: &str,
        values: &[(&'static str, String)],
        decision: Decision,
    ) -> Reply {
        let arg = |name: &str| {
            values
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let store = &self.pkgs;
        let switch = match row {
            "pkg_list" => return Reply::ok(store.list()),
            "pkg_search" => return Reply::ok(store.search(&arg("text"))),
            "pkg_install" => store.install(&[arg("name")]),
            "pkg_remove" => store.remove(&[arg("name")]),
            other => return Reply::err(format!("{other} is typed at the prompt for now")),
        };
        let switch = match switch {
            Ok(switch) => switch,
            Err(why) => return Reply::err(why),
        };
        let call = call(row, &[arg("name")]);
        match (decision.tier, decision.why, !switch.asks()) {
            // A vague line's reading is the AI's pick: an install it chose asks.
            (Tier::Run, Why::TakenAs, _) if row == "pkg_install" => {
                self.pkg_ask(id, request, &call, switch, Why::Added)
            }
            (Tier::Run, why, true) => {
                let consent = match why {
                    Why::TakenAs => "taken-as",
                    _ => "asked",
                };
                let how = How {
                    form: Form::Call,
                    tier: Tier::Run,
                    consent: String::from(consent),
                };
                self.pkg_switch(id, request, &call, switch, how)
            }
            (Tier::Run, _, false) => self.pkg_ask(id, request, &call, switch, Why::NoUndo),
            (_, why, _) => self.pkg_ask(id, request, &call, switch, why),
        }
    }

    /// Asks once about a switch, on the Allow / Deny menu; nothing changes yet.
    /// The card names what it installs and what that needs: its permissions,
    /// and the OS rows its packages' effects may call.
    fn pkg_ask(&mut self, id: u64, request: &str, call: &str, switch: Switch, why: Why) -> Reply {
        let needs = switch.needs();
        let (audit, subject) = match needs.is_empty() {
            true => (String::new(), None),
            false => {
                let names: Vec<String> = switch
                    .added
                    .iter()
                    .map(|p| format!("{} {}", p.name, p.version))
                    .collect();
                let subject = format!("{} ({needs})", names.join(", "));
                (format!("; {needs}"), Some(subject))
            }
        };
        self.host.audit(&format!(
            "request {id}: {call} asks ({}{audit})",
            switch.what()
        ));
        let reply = self.ask_call(id, request, Checked::shown_only(call), why, None, subject);
        if let Some(pending) = &mut self.pending {
            pending.pkg = Some(switch);
        }
        reply
    }

    /// The answer to a switch's menu: an Allow whose ONCE checks makes it.
    pub(crate) fn answer_pkg(&mut self, pending: Pending, text: &str) -> Reply {
        let id = pending.id;
        let Some(switch) = pending.pkg.clone() else {
            return Reply::err("nothing to confirm");
        };
        let checked = match (read_answer(text.trim()), &pending.menu) {
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
        if let Err(why) = checked {
            self.record(id, &pending.request, &pending.checked.script, "declined");
            self.remember(&pending, "Not run: declined");
            let why = if why.is_empty() {
                why
            } else {
                format!(": {why}")
            };
            return Reply::ok(format!("script {id} not run{why}"));
        }
        let how = How {
            consent: String::from("yes"),
            ..pending.how.clone()
        };
        let reply = self.pkg_switch(id, &pending.request, &pending.checked.script, switch, how);
        self.remember(&pending, &reply.text);
        reply
    }

    /// Makes a switch and logs it, with the generation switch as its undo.
    fn pkg_switch(
        &mut self,
        id: u64,
        request: &str,
        call: &str,
        switch: Switch,
        how: How,
    ) -> Reply {
        let what = switch.what();
        let to = switch.to.n;
        let from = match self.pkgs.commit(switch) {
            Ok(from) => from,
            Err(why) => return Reply::err(why),
        };
        self.host
            .audit(&format!("request {id}: packages generation {from} -> {to}"));
        let change = Change {
            action: String::from(call.split('(').next().unwrap_or("pkg")),
            owner: String::from("pkg"),
            res: String::from("pkg-generation"),
            before: format!("{from}"),
            after: format!("{to}"),
            inverse: Some(Inverse::Generation { from: to, to: from }),
        };
        self.log.push(Entry {
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
        Reply::ok(format!("Done: {what}. undo"))
    }

    /// `undo` of a package switch: back to generation `to` while `from` is current,
    /// or (`anyway`) whatever is.
    pub(crate) fn switch_generation(
        &mut self,
        from: u64,
        to: u64,
        anyway: bool,
    ) -> Result<String, Refused> {
        let done = format!("packages back to generation {to}");
        match self.pkgs.switch_back(from, to) {
            Ok(()) => Ok(done),
            Err(_) if anyway => self.pkgs.force(to).map(|()| done).map_err(Refused::Failed),
            Err(now) => Err(Refused::Moved(format!("generation {now}"))),
        }
    }
}

/// The canonical call a package line stands for: `pkg_install("notes")`.
fn call(action: &str, names: &[String]) -> String {
    let args: Vec<termcore::once::CallArg> = names
        .iter()
        .map(|n| termcore::once::CallArg::Text(n))
        .collect();
    termcore::once::call_text(action, &args).unwrap_or_else(|| String::from(action))
}
