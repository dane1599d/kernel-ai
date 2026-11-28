//! The test host's side of [`Host`]: what each call does on [`FakeHost`].

use super::*;

/// When the fake keeps a refused rules file: 2026-11-27 04:12 UTC.
pub(super) const KEPT_NS: u64 = 1_795_752_720_000_000_000;

impl FakeHost {
    pub(super) fn with_ledger<T>(&self, f: impl FnOnce(&mut crate::ledger::Ledger) -> T) -> T {
        let mut ledger = self.ledger.get();
        let out = f(&mut ledger);
        self.ledger.set(ledger);
        out
    }
}

impl Host for FakeHost {
    fn now_ns(&self) -> u64 {
        self.clock.set(self.clock.get() + self.step_ns.get());
        self.clock.get()
    }

    fn interrupted(&self) -> bool {
        if self.stopped.get() {
            return true;
        }
        match self.interrupt_in.get() {
            Some(0) => {
                self.interrupt_in.set(None);
                true
            }
            Some(n) => {
                self.interrupt_in.set(Some(n - 1));
                false
            }
            None => false,
        }
    }

    fn heap_bytes(&self) -> u64 {
        self.heap.set(self.heap.get() + self.heap_step.get());
        self.heap.get()
    }

    fn metrics(&self) -> Result<Metrics, kabi::Error> {
        self.fake_metrics()
    }

    fn set_policy(&self, id: u64) -> Result<(), kabi::Error> {
        if self.policy_busy.get() || self.busy_once.replace(false) {
            return Err(kabi::Error::Busy);
        }
        self.policy.set(Some(id));
        self.switches.set(self.switches.get() + 1);
        Ok(())
    }

    fn sleep_ms(&self, ms: u64) {
        self.slept.borrow_mut().push(ms);
    }

    fn ask(&self, prompt: &str) -> Result<String, String> {
        self.asked.borrow_mut().push(String::from(prompt));
        self.clock.set(self.clock.get() + self.ask_takes_ns);
        Ok(String::from("Hello from the fake LLM."))
    }

    fn script(&self, request: &ScriptRequest) -> Result<Answer, AnswerError> {
        self.script_requests.borrow_mut().push(request.clone());
        self.answers
            .borrow_mut()
            .pop_front()
            .unwrap_or(Err(AnswerError::Server(String::from("no more answers"))))
    }

    fn script_start(&self, request: &ScriptRequest) -> crate::appjob::Started {
        use crate::appjob::Started;
        if !self.background.get() {
            return Started::Inline;
        }
        if self.in_flight.get().is_some() {
            return Started::Busy(String::from("the model is still answering"));
        }
        self.script_requests.borrow_mut().push(request.clone());
        let ticket = self.tickets.get() + 1;
        self.tickets.set(ticket);
        self.in_flight.set(Some(ticket));
        Started::Ticket(ticket)
    }

    fn script_stop(&self, ticket: u64) {
        self.stops.borrow_mut().push(ticket);
    }

    fn model_warm(&self, lang: llmproto::script::Lang) {
        self.warms.borrow_mut().push(lang);
    }

    /// The answer comes once a test lets it ([`FakeHost::answered`]).
    fn script_take(&self) -> Option<(u64, Result<Answer, AnswerError>)> {
        if !self.answered.replace(false) {
            return None;
        }
        let ticket = self.in_flight.take()?;
        let answer = self.answers.borrow_mut().pop_front();
        Some((
            ticket,
            answer.unwrap_or(Err(AnswerError::Server(String::from("no more answers")))),
        ))
    }

    fn remember(&self, line: &str, call: &str, result: &str) {
        self.remembered
            .borrow_mut()
            .push((line.into(), call.into(), result.into()));
    }

    fn forget(&self) -> Result<(), String> {
        self.forgot.set(self.forgot.get() + 1);
        self.forget_refused.borrow().clone().map_or(Ok(()), Err)
    }

    fn once_check(&self, badge: u64, digest: &[u8; 32]) -> Result<(), String> {
        let mut book = self.once.borrow_mut();
        let entry = book
            .iter_mut()
            .find(|(b, _, _)| *b == badge)
            .ok_or_else(|| String::from("NotFound"))?;
        match entry {
            (_, _, true) => Err(String::from("NotFound")),
            (_, held, _) if held != digest => Err(String::from("Invalid")),
            (_, _, used) => {
                *used = true;
                Ok(())
            }
        }
    }

    fn stop_process(&self, name: &str, proof: Proof) -> Result<u64, String> {
        self.owner_check(&format!("process_stop(\"{name}\")"), proof)?;
        let mut procs = self.procs.borrow_mut();
        let before = procs.len();
        procs.retain(|p| p != name);
        match before - procs.len() {
            0 => Err(String::from("NotFound")),
            n => Ok(n as u64),
        }
    }

    fn stop_service(&self, name: &str, proof: Proof) -> Result<(), String> {
        match proof {
            // The supervisor's memory: only the entry that started it.
            Proof::Undo { id } => {
                self.proofs
                    .borrow_mut()
                    .push((format!("service_stop({name})"), proof));
                let started = self.started_by.borrow();
                if !started
                    .iter()
                    .rev()
                    .find(|(n, _)| n == name)
                    .is_some_and(|&(_, by)| by == id)
                {
                    return Err(String::from("err undo refused"));
                }
            }
            _ => self.owner_check(&format!("service_stop(\"{name}\")"), proof)?,
        }
        self.started_by.borrow_mut().retain(|(n, _)| n != name);
        self.set_running(name, false)
    }

    fn start_service(&self, name: &str, entry: u64) -> Result<(), String> {
        self.set_running(name, true)?;
        self.started_by.borrow_mut().push((name.into(), entry));
        Ok(())
    }

    fn desk_send_fits(&self) -> bool {
        self.send_fits.get()
    }

    fn desk_preview(&self, id: u64, text: &str) -> Result<(), String> {
        self.previews.borrow_mut().push((id, String::from(text)));
        Ok(())
    }

    fn net_send(&self, send: &crate::outbox::Send) -> Result<String, crate::outbox::SendFailed> {
        let disk = self.log_disk.borrow().clone();
        self.sends.borrow_mut().push((send.clone(), disk));
        self.send_answers
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Ok(String::from("200 OK")))
    }

    fn log_read(&self) -> Option<String> {
        self.log_disk.borrow().clone()
    }

    fn log_write(&self, text: &str) -> Result<(), LogFailed> {
        if self.log_fails.get() > 0 {
            self.log_fails.set(self.log_fails.get() - 1);
            return Err(LogFailed::Failed(String::from("the disk is full")));
        }
        // As `fs`: the open action's changes go, and its number is the next
        // commit's.
        if self.commit_refused.get()
            && let Some(open) = self.open_txn.take()
        {
            self.commit_refused.set(false);
            let (before, _) = self.txns.borrow_mut().remove(open);
            *self.files.borrow_mut() = before;
            let txn = open as u64 + 1;
            self.with_ledger(|l| {
                l.take_for_commit();
                l.commit_refused(txn);
            });
            return self.with_ledger(|l| {
                l.landed(Err(LogFailed::NotLanded {
                    txn,
                    why: String::from("Dropped(NoSpace)"),
                }))
            });
        }
        // A line that lost its action another way (a drop, never a rollback)
        // says so, as `Files::land` does.
        self.with_ledger(|l| l.take_for_commit());
        if let Some(txn) = self.open_txn.get() {
            self.with_ledger(|l| l.committed(txn as u64 + 1));
        }
        self.with_ledger(|l| l.landed(Ok(())))?;
        *self.log_disk.borrow_mut() = Some(text.into());
        self.log_writes.set(self.log_writes.get() + 1);
        self.open_txn.set(None);
        Ok(())
    }

    fn standing_read(&self) -> Result<Option<String>, String> {
        self.state_read("standing_read", &self.standing_disk)
    }

    fn standing_write(&self, text: &str) -> Result<(), String> {
        *self.standing_disk.borrow_mut() = Some(text.into());
        Ok(())
    }

    /// As the supervisor does: with `cap` a `mem` only lowers, and only a quota
    /// that changed has a `mem` from before.
    fn profile_apply(
        &self,
        settings: &[kaiprofile::Setting],
        cap: bool,
    ) -> Result<Vec<kaiprofile::Setting>, String> {
        let mut state = self.profile_state.borrow_mut();
        let mut before = Vec::new();
        for s in settings {
            let now = state
                .get(&s.process)
                .copied()
                .unwrap_or((kaiprofile::Class::Normal, 4096));
            let mem = s.mem.filter(|&pages| !cap || pages < now.1);
            before.push(kaiprofile::Setting {
                process: s.process.clone(),
                class: now.0,
                mem: mem.map(|_| now.1),
            });
            state.insert(s.process.clone(), (s.class, mem.unwrap_or(now.1)));
        }
        Ok(before)
    }

    fn focused_app(&self) -> Option<String> {
        self.focused.borrow().clone()
    }

    fn triggers(&self) -> Option<&dyn crate::standing::Triggers> {
        match self.triggers_on.get() {
            true => Some(&self.triggers),
            false => None,
        }
    }

    fn rules_read(&self) -> Result<Option<String>, String> {
        self.state_read("rules_read", &self.rules_disk)
    }

    fn rules_write(&self, text: &str) -> Result<(), String> {
        *self.rules_disk.borrow_mut() = Some(text.into());
        Ok(())
    }

    fn rules_set_aside(&self, text: &str) -> Result<String, String> {
        if !self.rules_aside_ok.get() {
            return Err(String::from("the disk did not answer"));
        }
        let mut aside = self.rules_aside.borrow_mut();
        let held = |name: &str| {
            Ok(aside
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, kept)| kept.clone()))
        };
        match crate::refused_rules_name(text, held) {
            Some(crate::SetAside::Kept(name)) => Ok(name),
            Some(crate::SetAside::Free(name)) => {
                aside.push((name.clone(), text.into()));
                Ok(name)
            }
            None => Err(crate::refused_rules_full()),
        }
    }

    fn refused_rules_read(&self, name: &str) -> Result<Option<(String, u64)>, String> {
        Ok(self
            .rules_aside
            .borrow()
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, kept)| (kept.clone(), KEPT_NS)))
    }

    fn refused_rules_trash(&self, name: &str) -> Result<u64, String> {
        if self.rules_trash_fails.get() {
            return Err(String::from("the disk did not answer"));
        }
        let mut aside = self.rules_aside.borrow_mut();
        let at = aside
            .iter()
            .position(|(n, _)| n == name)
            .ok_or_else(|| format!("{name}: no such file"))?;
        let mut trash = self.rules_trash.borrow_mut();
        trash.push(aside.remove(at));
        Ok(40 + trash.len() as u64)
    }

    /// The newest trashed copy of `name` back, unless the name is taken.
    fn refused_rules_untrash(&self, name: &str) -> Result<(), crate::FsRefused> {
        let mut aside = self.rules_aside.borrow_mut();
        if aside.iter().any(|(n, _)| n == name) {
            return Err(crate::FsRefused::Kept(format!("{name} exists again")));
        }
        let mut trash = self.rules_trash.borrow_mut();
        let at = trash
            .iter()
            .rposition(|(n, _)| n == name)
            .ok_or_else(|| crate::FsRefused::Kept(format!("no {name} in the trash")))?;
        aside.push(trash.remove(at));
        aside.sort();
        Ok(())
    }

    fn net_grants_read(&self) -> Result<Option<String>, String> {
        self.state_read("net_grants_read", &self.net_disk)
    }

    fn net_grants_write(&self, text: &str) -> Result<(), String> {
        if self.net_write_fails.get() > 0 {
            self.net_write_fails.set(self.net_write_fails.get() - 1);
            return Err(String::from("the disk did not answer"));
        }
        *self.net_disk.borrow_mut() = Some(text.into());
        Ok(())
    }

    fn withheld_read(&self) -> Result<Option<String>, String> {
        match self.withheld_error.borrow().clone() {
            Some(why) => Err(why),
            None => Ok(self.withheld_disk.borrow().clone()),
        }
    }

    fn withheld_write(&self, text: &str) -> Result<(), String> {
        *self.withheld_disk.borrow_mut() = Some(text.into());
        Ok(())
    }

    fn net_fetch(&self, host: &str, path: &str) -> Result<String, String> {
        self.net_seen
            .borrow_mut()
            .push(format!("net_fetch {host}{path}"));
        Ok(format!("<p>the page at {host}</p>"))
    }

    fn net_resolve(&self, host: &str) -> Result<Vec<String>, String> {
        self.net_seen
            .borrow_mut()
            .push(format!("net_resolve {host}"));
        Ok(alloc::vec![String::from("93.184.215.14")])
    }

    fn query(&self, request: &llmproto::query::QueryRequest) -> Result<String, String> {
        self.queries.borrow_mut().push(request.clone());
        self.query_answer.borrow().clone()
    }

    fn net_ping(&self, host: &str) -> Result<i64, String> {
        self.net_seen.borrow_mut().push(format!("net_ping {host}"));
        Ok(12)
    }

    fn fs_read(&self, read: FsRead) -> Result<String, String> {
        self.fake_fs_read(read)
    }

    fn fs_change(&self, change: FsChange) -> Result<u64, String> {
        self.fake_fs_change(change)
    }

    /// Restores transaction `txn` as kaifs does, file by file: a file that a
    /// later change touched is never restored over. One that existed before
    /// keeps its old text beside it as `name (restored)`; one the transaction
    /// made, or one a later change removed, is left. A restore that put any
    /// file back is a transaction of its own; one that put none back is refused.
    fn fs_undo(&self, txn: u64) -> Result<FsUndone, FsRefused> {
        self.fake_fs_undo(txn)
    }

    fn restart_service(&self, name: &str, proof: Proof) -> Result<(), String> {
        self.owner_check(&format!("service_restart(\"{name}\")"), proof)?;
        self.started_by.borrow_mut().retain(|(n, _)| n != name);
        self.set_running(name, false)?;
        self.set_running(name, true)
    }

    fn unix_seconds(&self) -> Option<u64> {
        self.wall_clock.get()
    }

    fn installed_rows(&self, package: &str) -> Result<String, String> {
        self.installed_asked
            .borrow_mut()
            .push(String::from(package));
        if let Some(why) = self.installed_down.borrow().clone() {
            return Err(why);
        }
        self.installed
            .borrow()
            .iter()
            .find(|(name, _)| name == package)
            .map(|(_, rows)| rows.clone())
            .ok_or_else(|| format!("{package} is not installed"))
    }

    fn services(&self) -> Result<Vec<crate::targets::Service>, String> {
        if !self.lists_services.get() {
            return Err(String::from("services cannot be listed here"));
        }
        let services = self.services.borrow();
        let listed =
            services
                .iter()
                .enumerate()
                .map(|(i, (name, running))| crate::targets::Service {
                    name: name.clone(),
                    pid: if *running { 20 + i as u64 } else { 0 },
                    stopped: !running,
                });
        Ok(listed.collect())
    }

    fn revoke_rule(&self, handle: u32) -> Result<(), String> {
        self.revoked.borrow_mut().push(handle);
        Ok(())
    }

    fn state(&self) -> crate::state::State {
        self.state.borrow().clone()
    }

    fn ui(&self, call: &crate::ui::UiCall) -> Result<String, String> {
        self.ui_calls.borrow_mut().push(call.clone());
        match self.ui_answer.borrow().as_ref() {
            Some(answer) => answer(call),
            None => crate::ui::no_desktop(),
        }
    }

    fn act_serving(
        &self,
        request: &ActRequest,
        serve: &mut dyn FnMut(crate::waiting::Waiting<'_>) -> Option<crate::waiting::Served>,
    ) -> Result<Call, ActError> {
        if let Some(serving) = self.serving.borrow().as_ref() {
            serving(serve);
        }
        self.act(request)
    }

    fn act(&self, request: &ActRequest) -> Result<Call, ActError> {
        self.act_lines.borrow_mut().push(request.line.clone());
        self.act_states.borrow_mut().push(request.state.clone());
        self.act_reads.borrow_mut().push(request.read.clone());
        self.act_apps.borrow_mut().push(request.apps.clone());
        self.calls
            .borrow_mut()
            .pop_front()
            .unwrap_or(Err(ActError::Unavailable(String::from("no more calls"))))
    }

    fn run_apart(&self, request: &str) -> Option<Result<String, String>> {
        let apart = self.apart.borrow();
        let child = apart.as_ref()?;
        self.apart_requests.borrow_mut().push(request.into());
        Some(child(request))
    }

    fn keep_read(&self) -> Result<Option<String>, String> {
        self.state_read("keep_read", &self.keep_file)
    }

    fn keep_write(&self, text: &str) -> Result<(), String> {
        *self.keep_file.borrow_mut() = Some(String::from(text));
        Ok(())
    }

    fn keep_running(&self, app: &str, on: bool) -> Result<(), String> {
        if self.keep_fails.get() > 0 {
            self.keep_fails.set(self.keep_fails.get() - 1);
            return Err(format!("cannot reach {app}: NotFound"));
        }
        if let Some(why) = self.keep_refusal.borrow().as_ref() {
            return Err(why.clone());
        }
        self.kept.borrow_mut().push((String::from(app), on));
        Ok(())
    }

    fn restart_read(&self) -> Result<Option<String>, String> {
        self.state_read("restart_read", &self.restart_file)
    }

    fn restart_write(&self, text: &str) -> Result<(), String> {
        *self.restart_file.borrow_mut() = Some(String::from(text));
        Ok(())
    }

    fn apps(&self) -> Option<&RefCell<crate::apps::Apps>> {
        Some(&self.apps)
    }

    fn app_call(&self, app: &str, request: &str) -> Result<String, String> {
        self.app_requests
            .borrow_mut()
            .push((String::from(app), String::from(request)));
        if self.not_apps.borrow().iter().any(|a| a == app) {
            return Err(String::from("no such app"));
        }
        if let Some(error) = self.app_errors.borrow_mut().pop_front() {
            return Err(error);
        }
        self.app_replies
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| String::from("the app did not answer"))
    }

    /// `updated`'s answers, as `models.rs` there gives them.
    fn models(&self, request: &str) -> Result<String, String> {
        self.fake_models(request)
    }

    fn appearance_read(&self) -> Result<Option<String>, String> {
        self.state_read("appearance_read", &self.appearance)
    }

    fn appearance_write(&self, text: &str) -> Result<(), String> {
        *self.appearance.borrow_mut() = Some(String::from(text));
        Ok(())
    }

    fn audit(&self, text: &str) {
        self.audit.borrow_mut().push(String::from(text));
    }

    fn power_reboot(&self) -> Result<(), String> {
        let log = self.log_disk.borrow().clone();
        self.reboots.borrow_mut().push(log);
        Ok(())
    }

    fn pkg(&self, request: &str) -> Result<String, String> {
        self.pkg_sent.borrow_mut().push(String::from(request));
        let answer = self.pkg_answers.borrow_mut().pop_front();
        answer.unwrap_or_else(|| Err(String::from("no pkg here")))
    }

    fn cc_build(
        &self,
        dir: &str,
        name: &str,
        version: &str,
    ) -> Result<crate::ccbuild::CcBuilt, String> {
        let at = match dir == crate::ccbuild::DIR {
            true => String::from(name),
            false => format!("{dir}/{name}"),
        };
        self.pkg_sent
            .borrow_mut()
            .push(format!("cc_build {at} {version}"));
        if let Some(built) = self.cc_builds.borrow_mut().pop_front() {
            return built;
        }
        let built = self.cc_built.borrow_mut().take();
        built.unwrap_or_else(|| Err(String::from("no cc here")))
    }

    fn cc_trial(&self, name: &str) -> Result<String, String> {
        self.pkg_sent.borrow_mut().push(format!("cc_trial {name}"));
        let next = self.cc_trials.borrow_mut().pop_front();
        next.unwrap_or_else(|| Ok(String::from(TRIAL_REPORT)))
    }

    fn cc_artifact(&self, _name: &str) -> Option<crate::ccbuild::Artifact> {
        self.cc_artifact.borrow().clone()
    }

    fn app_source(&self, name: &str, text: &str) -> Result<(), String> {
        self.app_sources
            .borrow_mut()
            .push((String::from(name), String::from(text)));
        Ok(())
    }

    fn app_source_read(&self, name: &str) -> Result<Option<String>, String> {
        let sources = self.app_sources.borrow();
        Ok(sources
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, text)| text.clone()))
    }

    fn pkg_local(&self, name: &str, path: &str, sha256: Option<&str>) -> Result<String, String> {
        match sha256 {
            Some(sha) => self.pkg(&format!("local {name} {path} sha256 {sha}")),
            None => self.pkg(&format!("local {name} {path}")),
        }
    }

    fn cc(&self, request: &[u8]) -> Result<Vec<u8>, String> {
        self.cc_sent
            .borrow_mut()
            .push(String::from_utf8_lossy(request).into_owned());
        if let Some(reply) = self.cc_script.borrow_mut().pop_front() {
            return Ok(reply);
        }
        match self.cc.borrow_mut().as_mut() {
            Some(service) => Ok(service.handle(request, &cmode::CcClock)),
            None => Err(String::from("cc is not running")),
        }
    }
}
