//! The Rhai shell: answers each line the kernel terminal sends with `ok <text>`,
//! `err <text>` or `confirm <id> <text>`. The evaluation itself lives in the
//! host-tested `shellcore` crate; this program connects it to the system calls.

#![no_std]
#![no_main]

extern crate alloc;

// A program's crate root sits in src/bin, so its modules are named by path.
#[path = "shell/apart.rs"]
mod apart;
#[path = "shell/files.rs"]
mod files;

use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use kabi::sched::{SchedStats, TrialRequest};
use llmproto::act::{self, ActError, ActRequest, Call};
use llmproto::chat::Completion;
use llmproto::script::{self, Answer, AnswerError, ScriptRequest};
use llmproto::tune::{self, TuneRequest};
use shellcore::targets::Service;
use shellcore::{DEFAULT_CONFIG, Host, Proof, SCRIPT_ROUND_MS, Shell};
use user::{arg, args, logln, println, sys};

user::entry!(main);

/// How long `ask` waits for the llm service.
const ASK_TIMEOUT_MS: u64 = 140_000;
/// How long one script round waits for the llm service: its own limit and a margin.
const SCRIPT_CALL_MS: u64 = SCRIPT_ROUND_MS + 5_000;
/// How long `remember` and `new` wait for llm, which only stores the turn.
const MEMORY_CALL_MS: u64 = 5_000;

/// How long a script's child may run: the line budget its engine keeps, and a
/// margin for the child's own check before it.
const SCRIPT_LIMIT_MS: u64 = DEFAULT_CONFIG.line_budget_ms + 2_000;

struct System {
    pool: apart::Pool,
    files: files::Files,
    state: files::State,
}

/// The route to the llm service: the shell's plan line has `uses=llm`, its only
/// route.
fn llm() -> kabi::handle::Handle {
    user::route(0).unwrap_or(kabi::handle::Handle::NONE)
}

/// The route to the supervisor: the second in the shell's `uses=llm+supervisor`.
fn supervisor() -> kabi::handle::Handle {
    user::route(1).unwrap_or(kabi::handle::Handle::NONE)
}

/// The supervisor's wait for a stop or a start: a stop waits for the process to end.
const SUPERVISOR_CALL_MS: u64 = 10_000;

/// Sends `stop`, `start` or `restart` to the supervisor: `ok`, or `err <why>`.
fn supervise(request: &str) -> Result<(), String> {
    let text = ask_supervisor(request)?;
    match text.split_once(' ').unwrap_or((&text, "")) {
        ("ok", _) => Ok(()),
        ("err", why) => Err(why.to_string()),
        _ => Err(String::from("the supervisor's answer was malformed")),
    }
}

/// A stop or restart carrying what lets it run, in the supervisor's own words
/// (`supervise::Request::encode`): `once=<badge> id=<entry>`, `rule`, or for a
/// stop, `undo id=<entry>`. One the supervisor would refuse is not sent.
fn with_proof(request: supervise::Request, proof: Proof) -> Result<String, String> {
    let proof = match proof {
        Proof::Once { badge, id } => supervise::Proof::Once(supervise::Once { badge, id }),
        Proof::Rule => supervise::Proof::Rule,
        Proof::Undo { id } => supervise::Proof::Undo(id),
        Proof::None => supervise::Proof::None,
    };
    supervise::proof_fits(request, proof)
        .map_err(|_| String::from("a stop or restart needs the menu's Allow or a rule"))?;
    Ok(request.encode(proof))
}

/// Sends `request` to the supervisor over the route; its answer.
fn ask_supervisor(request: &str) -> Result<String, String> {
    let mut reply = vec![0u8; 2048];
    let len = sys::call_route(
        supervisor(),
        request.as_bytes(),
        &mut reply,
        SUPERVISOR_CALL_MS,
    )
    .map_err(|error| format!("the supervisor did not answer ({error:?})"))?;
    let text = core::str::from_utf8(&reply[..len.min(reply.len())]).unwrap_or("err not text");
    Ok(text.to_string())
}

impl Host for System {
    fn now_ns(&self) -> u64 {
        sys::time_ns()
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

    fn act(&self, request: &ActRequest) -> Result<Call, ActError> {
        let unavailable = |why: &str| ActError::Unavailable(alloc::format!("llm service: {why}"));
        let mut reply = vec![0u8; kabi::MAX_MESSAGE];
        let len = sys::call_route(
            llm(),
            request.encode().as_bytes(),
            &mut reply,
            SCRIPT_CALL_MS,
        )
        .map_err(|error| unavailable(&alloc::format!("{error:?}")))?;
        let text = core::str::from_utf8(&reply[..len.min(reply.len())])
            .map_err(|_| unavailable("the reply is not UTF-8"))?;
        match text.split_once(' ').unwrap_or((text, "")) {
            ("ok", answer) => act::decode_answer(answer),
            ("err", why) => Err(unavailable(why)),
            _ => Err(unavailable("malformed reply")),
        }
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

    fn forget(&self) {
        let mut reply = vec![0u8; 256];
        let _ = sys::call_route(llm(), b"new", &mut reply, MEMORY_CALL_MS);
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

    /// One `profile` request to the supervisor per process (P3 task 12). One it
    /// refuses puts back those set before it, so a switch is all or nothing.
    fn profile_apply(
        &self,
        settings: &[kaiprofile::Setting],
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
        sys::grant_revoke(kabi::handle::Handle(handle))
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn log_read(&self) -> Option<String> {
        self.state.read(files::LOG)
    }

    /// In the same commit as the files the line changed (P2 task 12); else its
    /// own action. A log that fails to write still lets the change commit: the
    /// user was told it was done.
    fn log_write(&self, text: &str) -> Result<(), String> {
        if !self.files.is_open() {
            return self.state.write(files::LOG, text, self.files.next_action());
        }
        let logged = self.state.write_in(files::LOG, text);
        let committed = self.files.commit();
        logged.and(committed)
    }

    fn rules_read(&self) -> Option<String> {
        self.state.read(files::RULES)
    }

    fn standing_read(&self) -> Option<String> {
        self.state.read(files::STANDING)
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

    fn fs_read(&self, read: shellcore::FsRead) -> Result<String, String> {
        self.files.read(read)
    }

    fn fs_change(&self, change: shellcore::FsChange) -> Result<u64, String> {
        self.files.change(change)
    }

    fn fs_undo(&self, txn: u64, anyway: bool) -> Result<u64, shellcore::FsRefused> {
        self.files.undo(txn, anyway)
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
}

fn main() -> i32 {
    let args = args();
    let mut shell = Shell::new(
        {
            let (files, state) = files::Files::new();
            Rc::new(System {
                pool: apart::Pool::new(),
                files,
                state,
            })
        },
        DEFAULT_CONFIG,
    );
    println!("up (restart {})", arg(&args, "restart").unwrap_or("0"));
    let mut line = vec![0u8; kabi::MAX_MESSAGE];
    loop {
        let mut caller = u64::MAX;
        let len = match sys::svc_recv(&mut line, 0, &mut caller) {
            Ok(len) => len.min(line.len()),
            Err(kabi::Error::TimedOut) => continue,
            Err(error) => {
                println!("svc_recv failed: {error:?}");
                sys::sleep_ms(100);
                continue;
            }
        };
        // Only the kernel terminal may run shell lines.
        let mut reply = if caller == 0 {
            let text = core::str::from_utf8(&line[..len]).unwrap_or("");
            shell.eval(text).to_wire()
        } else {
            String::from("err the shell answers only the terminal")
        };
        reply.truncate(reply.floor_char_boundary(kabi::MAX_MESSAGE));
        if let Err(error) = sys::svc_reply(reply.as_bytes()) {
            println!("svc_reply failed: {error:?}");
        }
    }
}
