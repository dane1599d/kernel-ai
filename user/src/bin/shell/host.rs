//! The OS shell's [`Host`]: what shellcore asks of the system (the kernel's
//! calls, the services' routes, the network, files and the desktop), carried
//! out for it.

use super::*;

impl Host for System {
    fn now_ns(&self) -> u64 {
        sys::time_ns()
    }

    fn cc(&self, request: &[u8]) -> Result<Vec<u8>, String> {
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(cc_route(), request, &mut reply, CC_CALL_MS)
            .map_err(|error| format!("{error:?}"))?;
        reply.truncate(len.min(kabi::MAX_MESSAGE));
        Ok(reply)
    }

    fn pkg(&self, request: &str) -> Result<String, String> {
        build::ask_pkg(pkg_route()?, request)
    }

    fn unix_seconds(&self) -> Option<u64> {
        sys::clock_now().ok()
    }

    fn installed_rows(&self, package: &str) -> Result<String, String> {
        build::ask_pkg_long(pkg_route()?, &alloc::format!("actions {package}"))
    }

    fn package_installed(&self, package: &str) -> Result<bool, String> {
        let asked = build::ask_pkg_within(
            pkg_route()?,
            &alloc::format!("actions {package}"),
            build::PKG_QUICK_MS,
        );
        match asked {
            Ok(_) => Ok(true),
            Err(why) if why.ends_with(" is not installed") => Ok(false),
            Err(why) => Err(why),
        }
    }

    fn net_fetch(&self, host: &str, path: &str) -> Result<String, String> {
        net::fetch(host, path)
    }

    fn net_resolve(&self, host: &str) -> Result<Vec<String>, String> {
        net::resolve(host)
    }

    fn net_send(
        &self,
        send: &shellcore::outbox::Send,
    ) -> Result<String, shellcore::outbox::SendFailed> {
        net::send(send)
    }

    fn desk_send_fits(&self) -> bool {
        let Some(route) = desktop() else {
            return false;
        };
        matches!(
            ask_desktop(route, deskcore::sendview::FITS).as_deref(),
            Ok("yes")
        )
    }

    fn desk_preview(&self, id: u64, text: &str) -> Result<(), String> {
        let route = desktop().ok_or_else(|| String::from("there is no desktop"))?;
        let request = deskcore::sendview::encode_preview(id, text);
        match report_to_desktop(route, &request)? {
            answer if answer == "ok" => Ok(()),
            answer => Err(answer),
        }
    }

    fn cc_build(
        &self,
        dir: &str,
        name: &str,
        version: &str,
    ) -> Result<shellcore::ccbuild::CcBuilt, String> {
        build::build(&self.files, cc_route(), dir, name, version)
    }

    fn cc_trial(&self, name: &str) -> Result<String, String> {
        build::trial(cc_route(), name)
    }

    fn cc_artifact(&self, name: &str) -> Option<shellcore::ccbuild::Artifact> {
        build::artifact(shellcore::ccbuild::APPS_DIR, name)
    }

    fn app_source(&self, name: &str, text: &str) -> Result<(), String> {
        build::app_source(&self.files, name, text)
    }

    fn app_source_read(&self, name: &str) -> Result<Option<String>, String> {
        build::app_source_read(&self.files, name)
    }

    fn pkg_local(&self, name: &str, path: &str, sha256: Option<&str>) -> Result<String, String> {
        build::install(pkg_route()?, name, path, sha256)
    }

    fn models(&self, request: &str) -> Result<String, String> {
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(updated(), request.as_bytes(), &mut reply, UPDATED_CALL_MS)
            .map_err(|error| format!("the model channel did not answer ({error:?})"))?;
        let text = core::str::from_utf8(&reply[..len.min(reply.len())])
            .map_err(|_| String::from("the model channel's answer is not text"))?;
        Ok(text.to_string())
    }

    fn apps(&self) -> Option<&core::cell::RefCell<shellcore::apps::Apps>> {
        Some(&self.apps)
    }

    /// POWER_REBOOT with the plan's `power` (backlog item 13): it comes back
    /// only when the kernel did not restart. The kernel answers Busy before
    /// this boot's record is written and while `fs` serves a call, so a Busy is
    /// tried again every [`REBOOT_RETRY_MS`], up to [`REBOOT_TRIES`] times.
    fn power_reboot(&self) -> Result<(), String> {
        for _ in 0..REBOOT_TRIES {
            match sys::power_reboot() {
                kabi::Error::Busy => sys::sleep_ms(REBOOT_RETRY_MS),
                kabi::Error::Permission => {
                    return Err(String::from("this shell may not restart the computer"));
                }
                error => return Err(format!("the kernel said {error:?}")),
            }
        }
        Err(String::from("the computer is busy; try again in a moment"))
    }

    fn app_call(&self, app: &str, request: &str) -> Result<String, String> {
        ask_app(app, request)
    }

    fn interrupted(&self) -> bool {
        sys::interrupted()
    }

    fn heap_bytes(&self) -> u64 {
        user::heap_bytes()
    }

    fn metrics(&self) -> Result<kabi::Metrics, kabi::Error> {
        sys::metrics()
    }

    fn set_policy(&self, id: u64) -> Result<(), kabi::Error> {
        sys::set_policy(id).map(|_| ())
    }

    fn ask(&self, prompt: &str) -> Result<String, String> {
        let request = alloc::format!("ask {prompt}");
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(llm(), request.as_bytes(), &mut reply, ASK_TIMEOUT_MS)
            .map_err(|error| alloc::format!("llm service: {error:?}"))?;
        let text = String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned();
        match text.split_once(' ').unwrap_or((text.as_str(), "")) {
            ("ok", answer) => Ok(answer.to_string()),
            ("err", why) => Err(why.to_string()),
            _ => Err("llm service: malformed reply".to_string()),
        }
    }

    fn script(&self, request: &ScriptRequest) -> Result<Answer, AnswerError> {
        let server = |why: &str| AnswerError::Server(alloc::format!("llm service: {why}"));
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(
            llm(),
            request.encode().as_bytes(),
            &mut reply,
            SCRIPT_CALL_MS,
        )
        .map_err(|error| server(&alloc::format!("{error:?}")))?;
        let text = core::str::from_utf8(&reply[..len.min(reply.len())])
            .map_err(|_| server("the reply is not UTF-8"))?;
        match text.split_once(' ').unwrap_or((text, "")) {
            ("ok", answer) => {
                script::decode_answer(answer).ok_or_else(|| server("malformed answer"))
            }
            ("err", why) => Err(script::decode_error(why)),
            _ => Err(server("malformed reply")),
        }
    }

    /// `app-start` to llm (#618): a ticket, `inline` where rounds are not
    /// jobs, or `busy`. An llm that does not know the request builds inline.
    fn script_start(&self, request: &ScriptRequest) -> shellcore::appjob::Started {
        use core::sync::atomic::Ordering;
        use shellcore::appjob::Started;
        let asked = alloc::format!("app-start {}", request.encode());
        let text = match app_job(&asked) {
            Ok(text) => text,
            // llm may have taken the job and be slow to say so.
            Err(kabi::Error::TimedOut) => {
                return Started::Busy(String::from("the model did not answer in time"));
            }
            Err(_) => return Started::Inline,
        };
        match text.split_once(' ').unwrap_or((text.as_str(), "")) {
            ("ok", ticket) => match ticket.parse::<u64>() {
                Ok(ticket) if ticket != 0 => {
                    APP_TICKET.store(ticket, Ordering::Relaxed);
                    Started::Ticket(ticket)
                }
                _ => Started::Inline,
            },
            ("busy", why) => Started::Busy(why.to_string()),
            _ => Started::Inline,
        }
    }

    /// `app-take` for the job the shell waits for: nothing while it is
    /// `pending`, else its round's answer, or why it ended.
    fn script_take(&self) -> Option<(u64, Result<Answer, AnswerError>)> {
        use core::sync::atomic::Ordering;
        let ticket = APP_TICKET.load(Ordering::Relaxed);
        if ticket == 0 {
            return None;
        }
        // At most one app-take a pass of APP_TAKE_GAP_NS, however often
        // desktop events wake the loop.
        let now = sys::time_ns();
        if now.saturating_sub(APP_TAKEN_NS.load(Ordering::Relaxed)) < APP_TAKE_GAP_NS {
            return None;
        }
        APP_TAKEN_NS.store(now, Ordering::Relaxed);
        let gone = |why: &str| AnswerError::Unavailable(alloc::format!("llm service: {why}"));
        let answer = match app_job(&alloc::format!("app-take {ticket}")) {
            Ok(text) if text == "pending" => return None,
            Ok(text) => match text.split_once(' ').unwrap_or((text.as_str(), "")) {
                ("ok", answer) => {
                    script::decode_answer(answer).ok_or_else(|| gone("malformed answer"))
                }
                ("err", why) => Err(script::decode_error(why)),
                ("ended", why) => Err(gone(&alloc::format!("the build's job ended: {why}"))),
                _ => Err(gone("malformed reply")),
            },
            // A slow answer to app-take is asked again; a gone llm ends it.
            Err(kabi::Error::TimedOut) => return None,
            Err(error) => Err(gone(&alloc::format!("{error:?}"))),
        };
        APP_TICKET.store(0, Ordering::Relaxed);
        Some((ticket, answer))
    }

    fn script_stop(&self, ticket: u64) {
        use core::sync::atomic::Ordering;
        let _ = APP_TICKET.compare_exchange(ticket, 0, Ordering::Relaxed, Ordering::Relaxed);
        if let Err(error) = app_job(&alloc::format!("app-stop {ticket}")) {
            println!("app-stop {ticket}: {error:?}");
        }
    }

    fn model_warm(&self, lang: Lang) {
        let word = match lang {
            Lang::C => "c",
            Lang::Rhai | Lang::App => "rhai",
        };
        warmth::lang(word == "c");
        if let Err(error) = app_job(&alloc::format!("warm-lang {word}")) {
            println!("warm-lang {word}: {error:?}");
        }
    }

    fn act(&self, request: &ActRequest) -> Result<Call, ActError> {
        self.act_with(request, None)
    }

    fn act_serving(
        &self,
        request: &ActRequest,
        serve: &mut dyn FnMut(
            shellcore::waiting::Waiting<'_>,
        ) -> Option<shellcore::waiting::Served>,
    ) -> Result<Call, ActError> {
        self.act_with(request, Some(&mut || waiting::poll(&self.held, serve)))
    }

    /// A finished model turn for llm's conversation memory, in #55's wire form. An
    /// llm without memory answers that it does not know the request; that is fine.
    fn remember(&self, line: &str, call: &str, result: &str) {
        let request = alloc::format!(
            "remember {} {} {}
{line}{call}{result}",
            line.len(),
            call.len(),
            result.len()
        );
        let mut reply = vec![0u8; 256];
        let _ = sys::call_route(llm(), request.as_bytes(), &mut reply, MEMORY_CALL_MS);
    }

    fn forget(&self) -> Result<(), String> {
        // `new` empties llm's conversation, so it fills its slots again.
        warmth::cooled();
        let mut reply = vec![0u8; 256];
        let len = sys::call_route(llm(), b"new", &mut reply, MEMORY_CALL_MS)
            .map_err(|error| alloc::format!("llm did not answer ({error:?})"))?;
        // Lossy: the busy refusal names the app's first line, which the buffer
        // can cut inside a character.
        let text = String::from_utf8_lossy(&reply[..len.min(reply.len())]);
        if text.starts_with("ok") {
            return Ok(());
        }
        Err(String::from(text.strip_prefix("err ").unwrap_or(&text)))
    }

    /// The kernel checks the proof: the menu's ONCE, or (badge 0) a rule node
    /// in this table that covers the call (P1 task 14).
    fn stop_process(&self, name: &str, proof: Proof) -> Result<u64, String> {
        let (badge, id) = match proof {
            Proof::Once { badge, id } => (badge, id),
            Proof::Rule => (0, 0),
            Proof::None | Proof::Undo { .. } => {
                return Err(String::from("a stop needs the menu's Allow or a rule"));
            }
        };
        sys::proc_stop_once(name, badge, id).map_err(|error| format!("{error:?}"))
    }

    fn stop_service(&self, name: &str, proof: Proof) -> Result<(), String> {
        supervise(&with_proof(supervise::Request::Stop(name), proof)?)
    }

    /// The supervisor keeps the entry, so only that entry's undo may stop it unasked.
    fn start_service(&self, name: &str, entry: u64) -> Result<(), String> {
        let start = supervise::Request::Start(name);
        supervise(&start.encode(supervise::Proof::Entry(entry)))
    }

    fn restart_service(&self, name: &str, proof: Proof) -> Result<(), String> {
        supervise(&with_proof(supervise::Request::Restart(name), proof)?)
    }

    fn triggers(&self) -> Option<&dyn shellcore::standing::Triggers> {
        Some(&Firings)
    }

    /// One `profile` request to the supervisor per process (P3 task 12), with
    /// `cap` on a switch (#517). One it refuses puts back those set before it,
    /// so a switch is all or nothing.
    fn profile_apply(
        &self,
        settings: &[kaiprofile::Setting],
        cap: bool,
    ) -> Result<Vec<kaiprofile::Setting>, String> {
        let metrics = sys::metrics().map_err(|e| format!("cannot read the processes ({e:?})"))?;
        let pid_of = |name: &str| {
            metrics
                .processes
                .iter()
                .take(metrics.process_count as usize)
                .find(|p| p.name() == name)
                .map(|p| p.pid)
        };
        let mut done: Vec<(u64, supervise::profile::Set)> = Vec::new();
        let mut before = Vec::new();
        for setting in settings {
            let set = pid_of(&setting.process).map(|pid| supervise::profile::Set {
                pid,
                class: setting.class.number(),
                pages: setting.mem,
                cap,
            });
            let had = set
                .ok_or_else(|| format!("{} is not running", setting.process))
                .and_then(|set| {
                    let request = set.encode().ok_or("no such class")?;
                    supervise::profile::parse_before(set.pid, &ask_supervisor(&request)?)
                });
            match had {
                Ok(had) => {
                    done.push((had.pid, had));
                    before.push(kaiprofile::Setting {
                        process: setting.process.clone(),
                        class: match had.class {
                            1 => kaiprofile::Class::Background,
                            _ => kaiprofile::Class::Normal,
                        },
                        mem: had.pages,
                    });
                }
                Err(why) => {
                    for (_, had) in done.iter().rev() {
                        if let Some(request) = had.encode() {
                            let _ = ask_supervisor(&request);
                        }
                    }
                    return Err(format!("{}: {why}; nothing changed", setting.process));
                }
            }
        }
        Ok(before)
    }

    fn services(&self) -> Result<Vec<Service>, String> {
        let text = ask_supervisor(supervise::SERVICES)?;
        let listed = supervise::parse_listing(&text)
            .ok_or_else(|| format!("the supervisor's list was malformed: {text}"))?;
        let services = listed.into_iter().map(|s| Service {
            name: s.name,
            pid: s.pid,
            stopped: s.stopped,
        });
        Ok(services.collect())
    }

    fn once_check(&self, badge: u64, digest: &[u8; 32]) -> Result<(), String> {
        sys::once_check(badge, digest).map_err(|error| format!("{error:?}"))
    }

    fn revoke_rule(&self, handle: u32) -> Result<(), String> {
        let handle = kabi::handle::Handle(handle);
        let revoked = sys::grant_revoke(handle);
        // Its node gone, the handle can do nothing: closed, so a removed rule
        // keeps no table entry while the shell runs (bug hunt 3, H3-30).
        let _ = sys::handle_close(handle);
        revoked.map(|_| ()).map_err(|error| format!("{error:?}"))
    }

    fn log_read(&self) -> Option<String> {
        self.state.read(files::LOG).ok().flatten()
    }

    /// In the same commit as the files the line changed (P2 task 12); else its
    /// own action. A log that fails to write still lets the change commit: the
    /// user was told it was done. A line whose action `fs` ended without
    /// committing it, on whatever request, says so first
    /// ([`files::Files::land`]): the log went with it, or names it.
    fn log_write(&self, text: &str) -> Result<(), LogFailed> {
        if !self.files.is_open() {
            // Lost earlier in the line: this log would name its number,
            // which this very write would commit as.
            self.files.land()?;
            return self
                .state
                .write(files::LOG, text, self.files.next_action())
                .map_err(LogFailed::Failed);
        }
        // `fs` may save a held write here and drop the action, before the
        // Commit: `land` hears that too.
        let logged = self.state.write_in(files::LOG, text);
        self.files.land()?;
        logged.map_err(LogFailed::Failed)
    }

    fn rules_read(&self) -> Result<Option<String>, String> {
        self.state.read(files::RULES)
    }

    fn standing_read(&self) -> Result<Option<String>, String> {
        self.state.read(files::STANDING)
    }

    fn keep_read(&self) -> Result<Option<String>, String> {
        self.state.read(files::KEEP)
    }

    fn keep_write(&self, text: &str) -> Result<(), String> {
        match self.files.is_open() {
            true => self.state.write_in(files::KEEP, text),
            false => self
                .state
                .write(files::KEEP, text, self.files.next_action()),
        }
    }

    fn restart_read(&self) -> Option<String> {
        self.state.read(files::RESTART).ok().flatten()
    }

    /// As the keep-running list is: with the line's files, if it changed any.
    fn restart_write(&self, text: &str) -> Result<(), String> {
        match self.files.is_open() {
            true => self.state.write_in(files::RESTART, text),
            false => self
                .state
                .write(files::RESTART, text, self.files.next_action()),
        }
    }

    /// `keep <app> on|off` to the supervisor, which keeps the app from napping.
    fn keep_running(&self, app: &str, on: bool) -> Result<(), String> {
        supervise(&supervise::keep::encode(app, on))
    }

    fn appearance_read(&self) -> Option<String> {
        self.state.read(files::APPEARANCE).ok().flatten()
    }

    /// Saved with the line's files, as the rules are; then `desktop` paints in
    /// it at once ([`give_appearance`], between lines).
    fn appearance_write(&self, text: &str) -> Result<(), String> {
        match self.files.is_open() {
            true => self.state.write_in(files::APPEARANCE, text),
            false => self
                .state
                .write(files::APPEARANCE, text, self.files.next_action()),
        }?;
        self.appearance_due.set(true);
        self.appearance_tried_ns.set(0);
        Ok(())
    }

    /// As the rules are written: with the line's files, if it changed any.
    fn standing_write(&self, text: &str) -> Result<(), String> {
        match self.files.is_open() {
            true => self.state.write_in(files::STANDING, text),
            false => self
                .state
                .write(files::STANDING, text, self.files.next_action()),
        }
    }

    /// In the line's open action if it changed files, so the log line that
    /// follows lands with it; else its own action.
    fn rules_write(&self, text: &str) -> Result<(), String> {
        match self.files.is_open() {
            true => self.state.write_in(files::RULES, text),
            false => self
                .state
                .write(files::RULES, text, self.files.next_action()),
        }
    }

    /// At start, before any line: its own action.
    fn rules_set_aside(&self, text: &str) -> Result<(), String> {
        self.state
            .write(files::RULES_REFUSED, text, self.files.next_action())
    }

    fn net_grants_read(&self) -> Result<Option<String>, String> {
        self.state.read(files::NETGRANTS)
    }

    /// As the rules are written: with the line's files, if it changed any.
    fn net_grants_write(&self, text: &str) -> Result<(), String> {
        match self.files.is_open() {
            true => self.state.write_in(files::NETGRANTS, text),
            false => self
                .state
                .write(files::NETGRANTS, text, self.files.next_action()),
        }
    }

    fn withheld_read(&self) -> Result<Option<String>, String> {
        self.state.read(files::WITHHELD)
    }

    /// As the rules are written: with the line's files, if it changed any.
    fn withheld_write(&self, text: &str) -> Result<(), String> {
        match self.files.is_open() {
            true => self.state.write_in(files::WITHHELD, text),
            false => self
                .state
                .write(files::WITHHELD, text, self.files.next_action()),
        }
    }

    fn ui(&self, call: &shellcore::ui::UiCall) -> Result<String, String> {
        ui::call(call)
    }

    fn state(&self) -> shellcore::state::State {
        ui::state(shellcore::state::State::default())
    }

    fn fs_read(&self, read: shellcore::FsRead) -> Result<String, String> {
        self.files.read(read)
    }

    fn fs_change(&self, change: shellcore::FsChange) -> Result<u64, String> {
        self.files.change(change)
    }

    fn fs_undo(&self, txn: u64) -> Result<shellcore::FsUndone, shellcore::FsRefused> {
        self.files.undo(txn)
    }

    fn run_apart(&self, request: &str) -> Option<Result<String, String>> {
        self.pool.run(request, SCRIPT_LIMIT_MS)
    }

    /// The audit trail goes to the log, not the screen: the reply says what happened.
    fn audit(&self, text: &str) {
        logln!("{text}");
    }

    fn sched_stats(&self, window_ms: u64) -> Result<Box<SchedStats>, kabi::Error> {
        let mut stats = Box::new(SchedStats::empty());
        sys::sched_stats(&mut stats, window_ms)?;
        Ok(stats)
    }

    fn sched_trial(&self, request: &TrialRequest) -> Result<u64, kabi::Error> {
        sys::sched_trial(request)
    }

    fn set_spec(&self, id: u64, preset: u64) -> Result<(), kabi::Error> {
        sys::set_spec(id, preset).map(|_| ())
    }

    fn sleep_ms(&self, ms: u64) {
        sys::sleep_ms(ms);
    }

    fn tune(&self, request: &TuneRequest) -> Result<Completion, String> {
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(
            llm(),
            request.encode().as_bytes(),
            &mut reply,
            tune::CALL_TIMEOUT_MS,
        )
        .map_err(|error| alloc::format!("llm service: {error:?}"))?;
        let text = String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned();
        match text.split_once(' ').unwrap_or((text.as_str(), "")) {
            ("ok", completion) => tune::decode_completion(completion)
                .ok_or_else(|| "llm service: malformed answer".to_string()),
            ("err", why) => Err(why.to_string()),
            _ => Err("llm service: malformed reply".to_string()),
        }
    }

    /// A quarantined read (P5 task 11): `llm` answers it apart from every
    /// other request and checks the answer against its kind before replying.
    fn query(&self, request: &llmproto::query::QueryRequest) -> Result<String, String> {
        use llmproto::query;
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(
            llm(),
            request.encode().as_bytes(),
            &mut reply,
            query::CALL_TIMEOUT_MS,
        )
        .map_err(|error| alloc::format!("llm service: {error:?}"))?;
        query::decode_reply(&reply[..len.min(reply.len())])
    }
}
