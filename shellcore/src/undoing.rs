//! `undo`, `undo N` and `tune undo` over the action log (ADR 0002 D7).
//!
//! - `undo` reverts the newest live entry, its changes newest first. The undo is
//!   an entry of its own, so `undo` again redoes.
//! - `undo N` reverts entry N, but only if no later live entry changed the same
//!   resource; otherwise it names those entries.
//! - `tune undo` is `undo` of the newest kept trial.
//! - Every inverse is compare-and-set: it runs only if the resource still reads
//!   what the change left. If it moved, the user is asked (`y` puts it back
//!   anyway). A switch the kernel refuses as too soon is tried once more.

use crate::actlog::{Change, Entry, Form, State};
use crate::undo::{self, Refused};
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

impl Shell {
    /// `undo` with nothing (the newest live entry) or an entry number.
    pub(crate) fn undo_command(&mut self, which: &str) -> Reply {
        let which = which.trim().trim_start_matches('#');
        if which.is_empty() {
            // A send that went since the newest live entry: `undo` says so,
            // rather than taking back something older the person did not mean.
            if let Some(why) = self.bare_send_refusal() {
                return Reply::err(why);
            }
            return match self.log.newest_live() {
                Some(id) => self.revert_entry(id, false),
                None => Reply::err("nothing to undo"),
            };
        }
        let Ok(id) = which.parse::<u64>() else {
            return Reply::err("usage: undo [entry number]");
        };
        self.undo_entry(id)
    }

    /// `undo <agent>`: the newest live entry agent `agent` made, under the
    /// same checks as `undo <id>` (a later entry on what it changed asks).
    pub(crate) fn undo_agent(&mut self, agent: &str) -> Reply {
        let entries = self.log.entries().map(|e| (e.id, &e.agent, e.live()));
        match agentid::stamp::last_live(entries, agent) {
            Some(id) => self.undo_entry(id),
            None => Reply::err(format!("nothing of {agent}'s to undo")),
        }
    }

    /// `tune undo`: the newest trial the kernel kept.
    pub(crate) fn tune_undo(&mut self) -> Reply {
        match self.log.newest_live_of(Form::Tune) {
            Some(id) => self.undo_entry(id),
            None => Reply::err("nothing to undo: no trial kept a policy"),
        }
    }

    /// Entry `id`, if it is live and nothing later changed what it changed.
    fn undo_entry(&mut self, id: u64) -> Reply {
        match self.log.get(id) {
            Some(entry) if entry.live() => {}
            Some(entry) => {
                return Reply::err(
                    self.send_refusal(entry)
                        .unwrap_or_else(|| format!("#{id} has nothing to undo")),
                );
            }
            // A held send whose entry left the log's ring is still cancelled.
            None => {
                return match self.outbox.get(id).map(|p| p.send.host.clone()) {
                    Some(host) => match self.unsend(id, &host) {
                        Ok(text) => Reply::ok(format!("Undone: {text}")),
                        Err(why) => Reply::err(why),
                    },
                    None => Reply::err(format!("no logged entry #{id}")),
                };
            }
        }
        let later = self.log.later_touching(id);
        if !later.is_empty() {
            let names: Vec<String> = later.iter().map(|n| format!("#{n}")).collect();
            return Reply::err(format!(
                "{} changed the same thing since #{id}; undo {} first",
                names.join(", "),
                match later.len() {
                    1 => "it",
                    _ => "them",
                }
            ));
        }
        self.revert_entry(id, false)
    }

    /// After an undo took a local install out: that package's network grants
    /// go, so a later package of its name starts with none (ADR 0002 D3), and
    /// pkg is asked again about every grant holder.
    pub(crate) fn local_removed(&mut self, script: &str) {
        self.grant_memo.forget_installed();
        let host = self.host.as_ref();
        let Some(package) = crate::netask::local_package(script) else {
            host.audit("network grants kept: the undone install names no package");
            return;
        };
        // An undo of an upgrade leaves the older version installed: its
        // grants stay. A pkg that cannot say is taken as the package gone.
        let why = match host.package_installed(&package) {
            Ok(true) => return,
            Ok(false) => format!("the undo took {package} out"),
            Err(pkg) => format!(
                "the undo took {package} out, as far as is known (pkg could not say: {pkg})"
            ),
        };
        if let Err(not) = self.drop_package_grants(&package, &why) {
            self.host.audit(&format!(
                "{package}'s network grants were not dropped after its undo: {not}"
            ));
        }
    }

    /// Before a local install of `package`: if pkg says it is not installed,
    /// grants left under its name (an undo that could not drop them) go, so
    /// the new package starts with none.
    pub(crate) fn fresh_package(&mut self, package: &str) {
        let host = self.host.as_ref();
        match host.package_installed(package) {
            Ok(true) => {}
            Ok(false) => {
                let why = format!("{package} was not installed before this install");
                if let Err(not) = self.drop_package_grants(package, &why) {
                    self.host.audit(&format!(
                        "{package}'s old network grants were not dropped: {not}"
                    ));
                }
            }
            Err(pkg) => host.audit(&format!(
                "{package}'s old network grants, if any, were kept: pkg could not say whether it is installed ({pkg})"
            )),
        }
    }

    /// The answer to `undo`'s question about a value that moved since.
    pub(crate) fn undo_answer(&mut self, id: u64, yes: bool) -> Reply {
        match yes {
            true => self.revert_entry(id, true),
            false => Reply::ok("Not undone; `undo` asks again"),
        }
    }

    /// Puts entry `id`'s changes back, newest first, and logs the undo as an entry
    /// whose changes put them back again.
    pub(crate) fn revert_entry(&mut self, id: u64, anyway: bool) -> Reply {
        let Some(entry) = self.log.get(id).cloned() else {
            return Reply::err("nothing to undo");
        };
        let mut said = Vec::new();
        let mut redo: Vec<Change> = Vec::new();
        let mut left = entry.changes.clone();
        let mut stopped: Option<Reply> = None;
        // A script's file changes share one transaction (P2 task 11): the first
        // restore puts them all back, and its redo stands for each.
        let mut restored: Option<(u64, undo::Inverse)> = None;
        while let Some(change) = left.pop() {
            let Some(inverse) = change.inverse else {
                left.push(change);
                break;
            };
            if let (undo::Inverse::Fs { txn }, Some((done, again))) = (inverse, restored)
                && done == txn
            {
                redo.push(Change {
                    before: change.after.clone(),
                    after: change.before.clone(),
                    inverse: Some(again),
                    ..change
                });
                continue;
            }
            // A held send is cancelled; undoing that never sends it, so its
            // redo has no inverse: a send always asks again.
            if let undo::Inverse::Unsend { entry: held } = inverse {
                match self.unsend(held, &change.res) {
                    Ok(text) => {
                        said.push(text);
                        redo.push(Change {
                            before: change.after.clone(),
                            after: String::from(crate::outbox::Stage::Cancelled.word()),
                            inverse: None,
                            ..change
                        });
                        continue;
                    }
                    Err(why) => {
                        left.push(change);
                        stopped = Some(Reply::err(format!("{}{why}", undone(&said))));
                        break;
                    }
                }
            }
            let reverted = match inverse {
                undo::Inverse::Generation { from, to } => self
                    .switch_generation(from, to, anyway)
                    .map(|text| (text, undo::reverse(inverse))),
                undo::Inverse::Instruction { id: rule, made } => self
                    .revert_instruction(rule, made)
                    .map(|text| (text, undo::reverse(inverse)))
                    .map_err(Refused::Failed),
                undo::Inverse::Keep { app, on } => self
                    .revert_keep(app.as_str(), on)
                    .map(|text| (text, undo::reverse(inverse)))
                    .map_err(Refused::Failed),
                undo::Inverse::Profile { key } => self
                    .revert_profile(key)
                    .map(|(text, redo)| (text, undo::Inverse::Profile { key: redo }))
                    .map_err(Refused::Failed),
                undo::Inverse::NetGrant { id: grant, held } => self
                    .revert_net_grant(grant, held, &change.res)
                    .map(|text| (text, undo::reverse(inverse)))
                    .map_err(Refused::Failed),
                undo::Inverse::Model { back, from } => self
                    .model_back(back, from, anyway)
                    .map(|text| (text, undo::reverse(inverse))),
                undo::Inverse::Local { from, to } if to < from => {
                    let ids = undo::Ids {
                        made: id,
                        next: self.next_id,
                    };
                    let done = undo::revert(self.host.as_ref(), inverse, anyway, ids);
                    if done.is_ok() {
                        self.local_removed(&entry.script);
                    }
                    done
                }
                other => {
                    let ids = undo::Ids {
                        made: id,
                        next: self.next_id,
                    };
                    undo::revert(self.host.as_ref(), other, anyway, ids)
                }
            };
            match reverted {
                Ok((text, again)) => {
                    if let undo::Inverse::Fs { txn } = inverse {
                        restored = Some((txn, again));
                    }
                    said.push(text);
                    redo.push(Change {
                        before: change.after.clone(),
                        after: change.before.clone(),
                        inverse: Some(again),
                        ..change
                    });
                }
                Err(Refused::Moved(now)) => {
                    self.undo_asking = Some(id);
                    self.host
                        .audit(&format!("undo #{id} asks: changed since (now {now})"));
                    let mut text = undone(&said);
                    text.push_str(&format!(
                        "The {} changed since (now {now}); put {} back anyway? y/n",
                        change.res,
                        undo::target(inverse)
                    ));
                    left.push(change);
                    stopped = Some(Reply::ok(text));
                    break;
                }
                Err(Refused::Failed(why)) => {
                    self.host.audit(&format!("undo #{id} failed: {why}"));
                    left.push(change);
                    stopped = Some(Reply::err(format!(
                        "{}{why}; try undo again",
                        undone(&said)
                    )));
                    break;
                }
            }
        }
        if !redo.is_empty() {
            self.log_undo(&entry, left, redo);
        }
        stopped.unwrap_or_else(|| Reply::ok(format!("Undone: {}", said.join("; "))))
    }

    /// The undo of `entry` as an entry: what it put back, and what of `entry` is
    /// still to undo.
    fn log_undo(&mut self, entry: &Entry, left: Vec<Change>, redo: Vec<Change>) {
        let id = self.next_id;
        self.next_id += 1;
        if let Some(original) = self.log.get_mut(entry.id) {
            original.changes = left;
            if original.changes.is_empty() {
                original.state = State::Undone;
            }
        }
        self.host.audit(&format!("undo #{id}: undid #{}", entry.id));
        self.log.push(Entry {
            agent: agentid::Name::kai(),
            id,
            t_ns: self.host.now_ns(),
            form: Form::Undo,
            of: Some(entry.id),
            tier: None,
            pack: None,
            consent: String::from("typed"),
            state: State::Done,
            request: String::from("undo"),
            script: String::new(),
            outcome: String::from("done"),
            changes: redo,
        });
    }

    /// Logs a trial the kernel kept, if one was, so `undo` can put it back.
    pub(crate) fn log_kept_trial(&mut self) {
        let Some(kept) = self.tuner.take_kept() else {
            return;
        };
        let (before, after) = (kept.before, kept.after);
        let change = Change {
            action: String::from("tune"),
            owner: String::from("kernel"),
            res: String::from("policy"),
            before: undo::spec_name(before.0, before.1),
            after: undo::spec_name(after.0, after.1),
            inverse: Some(undo::Inverse::Spec {
                policy: before.0,
                preset: before.1,
                after,
            }),
        };
        self.log.push(Entry {
            agent: agentid::Name::kai(),
            id: kept.id,
            t_ns: self.host.now_ns(),
            form: Form::Tune,
            of: None,
            tier: None,
            pack: None,
            consent: String::from("yes"),
            state: State::Done,
            request: format!("tune {}", kept.target),
            script: String::new(),
            outcome: format!("kept {}", change.after),
            changes: alloc::vec![change],
        });
    }
}

/// `Undone: …` and a newline when something was put back before a stop.
fn undone(said: &[String]) -> String {
    match said.is_empty() {
        true => String::new(),
        false => format!("Undone: {}\n", said.join("; ")),
    }
}
