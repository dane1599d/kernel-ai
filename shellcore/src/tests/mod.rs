extern crate std;

use super::*;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use kabi::{Metrics, policy};
use llmproto::act::{ActError, ActRequest, Call};
use llmproto::script::{Answer, AnswerError, ScriptRequest};
use std::collections::VecDeque;

mod act;
mod actions;
mod apart;
mod files;
mod fuzz;
mod log;
mod logdisk;
mod more;
mod packages;
mod panel;
mod profiles;
mod scripts;
mod standing;
mod transaction;
mod tune;
mod ui;

/// A host whose clock moves `step_ns` on every reading.
struct FakeHost {
    clock: Cell<u64>,
    step_ns: Cell<u64>,
    /// Interrupt after this many polls.
    interrupt_in: Cell<Option<u32>>,
    heap: Cell<u64>,
    /// Added to `heap` on every reading, to make a run look as if it allocates.
    heap_step: Cell<u64>,
    policy: Cell<Option<u64>>,
    /// Set to make policy switches fail, as the kernel's rate limit does.
    policy_busy: Cell<bool>,
    /// Set to make the next policy switch fail as too soon, and only that one.
    busy_once: Cell<bool>,
    /// Policy switches made.
    switches: Cell<u32>,
    /// Every `sleep_ms`, in order.
    slept: RefCell<Vec<u64>>,
    asked: RefCell<Vec<String>>,
    ask_takes_ns: u64,
    /// Answers `script` gives, in order; afterwards it fails.
    answers: RefCell<VecDeque<Result<Answer, AnswerError>>>,
    script_requests: RefCell<Vec<ScriptRequest>>,
    /// Calls `act` gives, in order; afterwards there is none.
    calls: RefCell<VecDeque<Result<Call, ActError>>>,
    act_lines: RefCell<Vec<String>>,
    /// The State each `act` request carried, and what `state` gives.
    act_states: RefCell<Vec<String>>,
    /// Each act request's second-round read (P3 task 10).
    act_reads: RefCell<Vec<Option<llmproto::act::Read>>>,
    /// What the shell asked `desktop`, and how `desktop` answers.
    ui_calls: RefCell<Vec<crate::ui::UiCall>>,
    ui_answer: RefCell<Option<UiAnswer>>,
    state: RefCell<crate::state::State>,
    /// Turns sent to the conversation memory, and how often it was forgotten.
    remembered: RefCell<Vec<(String, String, String)>>,
    forgot: Cell<u32>,
    audit: RefCell<Vec<String>>,
    /// The trusted path's ONCE book: badge, digest, and whether it was used.
    once: RefCell<Vec<(u64, [u8; 32], bool)>>,
    /// Rule handles revoked.
    revoked: RefCell<Vec<u32>>,
    /// Running processes by name, and services with whether they run.
    procs: RefCell<Vec<String>>,
    services: RefCell<Vec<(String, bool)>>,
    /// Whether the supervisor lists its services, and metrics show the processes
    /// above (services from pid 20, plain processes from pid 30).
    lists_services: Cell<bool>,
    /// What a `scriptrun` child does with a request; none runs scripts here.
    apart: RefCell<Option<Apart>>,
    /// Requests sent to a child.
    apart_requests: RefCell<Vec<String>>,
    /// What each stop, start and restart showed its owner, in order.
    proofs: RefCell<Vec<(String, Proof)>>,
    /// Services started, with the entry that started them: the supervisor's memory.
    started_by: RefCell<Vec<(String, u64)>>,
    /// The `fs` server's files, and each transaction's files before and after.
    files: RefCell<std::collections::BTreeMap<String, String>>,
    txns: RefCell<Vec<(Files, Files)>>,
    /// As the shell does on `fs`: a line's changes are one transaction, left open
    /// until the log write commits it; the open one's index.
    one_action: Cell<bool>,
    open_txn: Cell<Option<usize>>,
    /// `/system/log`, and how often it was written.
    log_disk: RefCell<Option<String>>,
    log_writes: Cell<u32>,
    /// `/system/state/rules`.
    rules_disk: RefCell<Option<String>>,
    /// `/system/state/standing`, and the event source and owners standing
    /// instructions run through, when `triggers_on` is set.
    standing_disk: RefCell<Option<String>>,
    triggers_on: Cell<bool>,
    triggers: FakeTriggers,
    /// What the supervisor set each process to (class, page quota); a process
    /// not here runs normal with 4,096 pages. And the app with focus.
    profile_state: RefCell<std::collections::BTreeMap<String, (kaiprofile::Class, u64)>>,
    focused: RefCell<Option<String>>,
}

/// What standing instructions asked the desktop and the owners for.
#[derive(Default)]
pub(super) struct FakeTriggers {
    pub watched: RefCell<Vec<crate::standing::When>>,
    pub runs: RefCell<Vec<(u64, String, String, bool)>>,
    /// The owner's answer: `Err` when it finds no live rule covering a firing.
    pub refuse: RefCell<Option<String>>,
}

impl crate::standing::Triggers for FakeTriggers {
    fn watch(&self, when: &crate::standing::When) -> Result<(), String> {
        self.watched.borrow_mut().push(when.clone());
        Ok(())
    }

    fn unwatch(&self, when: &crate::standing::When) {
        self.watched.borrow_mut().retain(|w| w != when);
    }

    fn run(&self, rule: u64, action: &str, app: &str, undo: bool) -> Result<(), String> {
        if let Some(why) = self.refuse.borrow().clone() {
            return Err(why);
        }
        self.runs
            .borrow_mut()
            .push((rule, action.into(), app.into(), undo));
        Ok(())
    }
}

type Files = std::collections::BTreeMap<String, String>;

type Apart = alloc::boxed::Box<dyn Fn(&str) -> Result<String, String>>;

type UiAnswer = alloc::boxed::Box<dyn Fn(&crate::ui::UiCall) -> Result<String, String>>;

impl FakeHost {
    /// The supervisor's side: `err already ...` when nothing changes.
    fn set_running(&self, name: &str, running: bool) -> Result<(), String> {
        let mut services = self.services.borrow_mut();
        let (_, state) = services
            .iter_mut()
            .find(|(n, _)| n == name)
            .ok_or_else(|| String::from("unknown service"))?;
        if *state == running {
            return Err(String::from(if running {
                "not stopped"
            } else {
                "already stopped"
            }));
        }
        *state = running;
        Ok(())
    }

    /// What a stop's owner checks (the kernel, the supervisor): the menu's ONCE
    /// against `call` as shown, used up by a match, or a standing rule. With no
    /// proof it refuses, as the real owners do.
    fn owner_check(&self, call: &str, proof: Proof) -> Result<(), String> {
        self.proofs.borrow_mut().push((call.into(), proof));
        match proof {
            Proof::Once { badge, id } => self
                .once_check(badge, &termcore::once::digest_call(id, call))
                .map_err(|why| format!("ONCE {why}")),
            Proof::Rule => Ok(()),
            Proof::None | Proof::Undo { .. } => Err(String::from("Permission")),
        }
    }

    /// What the kernel terminal does on Allow: a ONCE badge for menu `id`'s
    /// `call`, as shown.
    fn mint(&self, id: u64, call: &str) -> u64 {
        let mut book = self.once.borrow_mut();
        let badge = 0x0ce0 + book.len() as u64;
        book.push((badge, termcore::once::digest_call(id, call), false));
        badge
    }
}

impl FakeHost {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            clock: Cell::new(0),
            step_ns: Cell::new(1000),
            interrupt_in: Cell::new(None),
            heap: Cell::new(0),
            heap_step: Cell::new(0),
            policy: Cell::new(None),
            policy_busy: Cell::new(false),
            busy_once: Cell::new(false),
            switches: Cell::new(0),
            slept: RefCell::new(Vec::new()),
            asked: RefCell::new(Vec::new()),
            ask_takes_ns: 0,
            answers: RefCell::new(VecDeque::new()),
            script_requests: RefCell::new(Vec::new()),
            calls: RefCell::new(VecDeque::new()),
            act_lines: RefCell::new(Vec::new()),
            act_states: RefCell::new(Vec::new()),
            act_reads: RefCell::new(Vec::new()),
            ui_calls: RefCell::new(Vec::new()),
            ui_answer: RefCell::new(None),
            state: RefCell::new(crate::state::State::default()),
            remembered: RefCell::new(Vec::new()),
            forgot: Cell::new(0),
            audit: RefCell::new(Vec::new()),
            once: RefCell::new(Vec::new()),
            revoked: RefCell::new(Vec::new()),
            procs: RefCell::new(alloc::vec![
                String::from("build-3"),
                String::from("build-4")
            ]),
            services: RefCell::new(alloc::vec![(String::from("notes"), false)]),
            lists_services: Cell::new(false),
            apart: RefCell::new(None),
            apart_requests: RefCell::new(Vec::new()),
            proofs: RefCell::new(Vec::new()),
            started_by: RefCell::new(Vec::new()),
            files: RefCell::new(Files::new()),
            txns: RefCell::new(Vec::new()),
            one_action: Cell::new(false),
            open_txn: Cell::new(None),
            log_disk: RefCell::new(None),
            log_writes: Cell::new(0),
            rules_disk: RefCell::new(None),
            standing_disk: RefCell::new(None),
            triggers_on: Cell::new(false),
            triggers: FakeTriggers::default(),
            profile_state: RefCell::new(std::collections::BTreeMap::new()),
            focused: RefCell::new(None),
        })
    }
}

impl Host for FakeHost {
    fn now_ns(&self) -> u64 {
        self.clock.set(self.clock.get() + self.step_ns.get());
        self.clock.get()
    }

    fn interrupted(&self) -> bool {
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
        let mut m = Metrics::empty();
        m.cpus = 4;
        m.free_frames = 256 * 100;
        m.total_frames = 256 * 128;
        m.process_count = 1;
        m.processes[0].pid = 7;
        m.processes[0].name[..3].copy_from_slice(b"llm");
        m.processes[0].name_len = 3;
        if self.lists_services.get() {
            let services = self.services.borrow();
            let running = services
                .iter()
                .enumerate()
                .filter(|(_, (_, running))| *running)
                .map(|(i, (name, _))| (20 + i as u64, name.clone()));
            let procs = self.procs.borrow();
            let plain = procs
                .iter()
                .enumerate()
                .map(|(i, name)| (30 + i as u64, name.clone()));
            for (pid, name) in running.chain(plain) {
                let slot = &mut m.processes[m.process_count as usize];
                slot.pid = pid;
                slot.name[..name.len()].copy_from_slice(name.as_bytes());
                slot.name_len = name.len() as u64;
                m.process_count += 1;
            }
        }
        m.policy = self.policy.get().unwrap_or(policy::INTERACTIVE);
        m.running_policy = m.policy;
        Ok(m)
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

    fn remember(&self, line: &str, call: &str, result: &str) {
        self.remembered
            .borrow_mut()
            .push((line.into(), call.into(), result.into()));
    }

    fn forget(&self) {
        self.forgot.set(self.forgot.get() + 1);
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

    fn log_read(&self) -> Option<String> {
        self.log_disk.borrow().clone()
    }

    fn log_write(&self, text: &str) -> Result<(), String> {
        *self.log_disk.borrow_mut() = Some(text.into());
        self.log_writes.set(self.log_writes.get() + 1);
        self.open_txn.set(None);
        Ok(())
    }

    fn standing_read(&self) -> Option<String> {
        self.standing_disk.borrow().clone()
    }

    fn standing_write(&self, text: &str) -> Result<(), String> {
        *self.standing_disk.borrow_mut() = Some(text.into());
        Ok(())
    }

    fn profile_apply(
        &self,
        settings: &[kaiprofile::Setting],
    ) -> Result<Vec<kaiprofile::Setting>, String> {
        let mut state = self.profile_state.borrow_mut();
        let mut before = Vec::new();
        for s in settings {
            let now = state
                .get(&s.process)
                .copied()
                .unwrap_or((kaiprofile::Class::Normal, 4096));
            before.push(kaiprofile::Setting {
                process: s.process.clone(),
                class: now.0,
                mem: Some(now.1),
            });
            state.insert(s.process.clone(), (s.class, s.mem.unwrap_or(now.1)));
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

    fn rules_read(&self) -> Option<String> {
        self.rules_disk.borrow().clone()
    }

    fn rules_write(&self, text: &str) -> Result<(), String> {
        *self.rules_disk.borrow_mut() = Some(text.into());
        Ok(())
    }

    fn fs_read(&self, read: FsRead) -> Result<String, String> {
        let files = self.files.borrow();
        match read {
            FsRead::List(dir) => Ok(files
                .keys()
                .filter(|path| path.starts_with(dir))
                .cloned()
                .collect::<Vec<_>>()
                .join(
                    "
",
                )),
            FsRead::Read(path) => files
                .get(path)
                .cloned()
                .ok_or_else(|| String::from("NotFound")),
            FsRead::History(path) => Ok(format!("{path}: {} versions", self.txns.borrow().len())),
            FsRead::Changes => Ok(files.keys().cloned().collect::<Vec<_>>().join(
                "
",
            )),
        }
    }

    fn fs_change(&self, change: FsChange) -> Result<u64, String> {
        let before = self.files.borrow().clone();
        let mut files = before.clone();
        match change {
            FsChange::Create { path, .. } if files.contains_key(path) => {
                return Err(String::from("Exists: use fs_write"));
            }
            FsChange::Create { path, text } | FsChange::Write { path, text } => {
                files.insert(path.into(), text.into());
            }
            FsChange::Move { from, to } => {
                let text = files.remove(from).ok_or_else(|| String::from("NotFound"))?;
                files.insert(to.into(), text);
            }
            FsChange::Trash(path) => {
                let text = files.remove(path).ok_or_else(|| String::from("NotFound"))?;
                files.insert(format!(".trash/{path}"), text);
            }
            FsChange::Restore(path) => {
                let text = files
                    .remove(&format!(".trash/{path}"))
                    .ok_or_else(|| String::from("NotFound"))?;
                files.insert(path.into(), text);
            }
            FsChange::EmptyTrash => files.retain(|path, _| !path.starts_with(".trash/")),
        }
        *self.files.borrow_mut() = files.clone();
        let mut txns = self.txns.borrow_mut();
        if let Some(open) = self.open_txn.get() {
            txns[open].1 = files;
            return Ok(open as u64 + 1);
        }
        txns.push((before, files));
        if self.one_action.get() {
            self.open_txn.set(Some(txns.len() - 1));
        }
        Ok(txns.len() as u64)
    }

    fn fs_undo(&self, txn: u64, anyway: bool) -> Result<u64, FsRefused> {
        let (before, after) = self
            .txns
            .borrow()
            .get(txn as usize - 1)
            .cloned()
            .ok_or_else(|| FsRefused::Failed(String::from("NotFound")))?;
        let now = self.files.borrow().clone();
        // The line's own open transaction is dropped, not restored.
        if self.open_txn.get() == Some(txn as usize - 1) {
            self.open_txn.set(None);
            *self.files.borrow_mut() = before;
            return Ok(txn);
        }
        if now != after && !anyway {
            return Err(FsRefused::Later(String::from(
                "a later change touched them",
            )));
        }
        *self.files.borrow_mut() = before.clone();
        let mut txns = self.txns.borrow_mut();
        txns.push((now, before));
        Ok(txns.len() as u64)
    }

    fn restart_service(&self, name: &str, proof: Proof) -> Result<(), String> {
        self.owner_check(&format!("service_restart(\"{name}\")"), proof)?;
        self.started_by.borrow_mut().retain(|(n, _)| n != name);
        self.set_running(name, false)?;
        self.set_running(name, true)
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

    fn act(&self, request: &ActRequest) -> Result<Call, ActError> {
        self.act_lines.borrow_mut().push(request.line.clone());
        self.act_states.borrow_mut().push(request.state.clone());
        self.act_reads.borrow_mut().push(request.read.clone());
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

    fn audit(&self, text: &str) {
        self.audit.borrow_mut().push(String::from(text));
    }
}

fn shell(host: &Rc<FakeHost>) -> Shell {
    Shell::new(Rc::clone(host) as Rc<dyn Host>, DEFAULT_CONFIG)
}

fn ok(text: &str) -> Reply {
    Reply {
        ok: true,
        text: String::from(text),
        confirm: None,
        more: false,
    }
}

#[test]
fn variables_and_functions_persist_and_values_are_echoed() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(shell.eval("let x = 40"), ok(""));
    assert_eq!(shell.eval("fn add(a, b) { a + b }"), ok(""));
    assert_eq!(shell.eval("add(x, 2)"), ok("42"));
    assert_eq!(shell.eval("\"text\""), ok("text"));
    assert_eq!(shell.eval("[1, \"a\", #{b: 2}]"), ok("[1, \"a\", #{b: 2}]"));
}

#[test]
fn earlier_lines_do_not_run_again() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(shell.eval("print(\"once\"); 1"), ok("once\n1"));
    assert_eq!(shell.eval("2"), ok("2"));
}

#[test]
fn a_runaway_line_hits_the_time_limit_and_the_shell_carries_on() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.eval("let x = 40");
    host.step_ns.set(100_000_000);
    let reply = shell.eval("loop { }");
    assert!(!reply.ok);
    assert_eq!(reply.text, "stopped: time limit (2000 ms) reached");
    host.step_ns.set(1000);
    assert_eq!(shell.eval("x"), ok("40"));
}

#[test]
fn ctrl_c_and_memory_stop_a_line() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.interrupt_in.set(Some(5));
    assert_eq!(shell.eval("loop { }").text, "stopped: interrupted");
    host.heap_step.set(DEFAULT_CONFIG.heap_budget / 4);
    assert!(
        shell
            .eval("loop { }")
            .text
            .starts_with("stopped: memory budget")
    );
}

#[test]
fn an_old_ctrl_c_does_not_stop_the_next_line() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.interrupt_in.set(Some(0));
    assert_eq!(
        shell.eval("let n = 0; for i in 0..5000 { n += 1 } n"),
        ok("5000")
    );
}

#[test]
fn deep_recursion_and_nesting_are_errors_not_crashes() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.eval("let x = 1");
    assert!(!shell.eval("fn deep(n) { deep(n + 1) } deep(0)").ok);
    let nested = std::format!("{}1{}", "[".repeat(64), "]".repeat(64));
    assert!(!shell.eval(&nested).ok);
    assert_eq!(shell.eval("x"), ok("1"));
}

#[test]
fn deep_values_are_shown_cut_off() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(shell.eval("[[[[[[1]]]]]]"), ok("[[[[[...]]]]]"));
}

#[test]
fn printed_output_is_capped() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let reply = shell.eval("for i in 0..2000 { print(\"line \" + i) }");
    assert!(reply.ok, "{reply:?}");
    assert!(reply.text.len() < DEFAULT_CONFIG.max_output + 100);
    assert!(reply.text.contains("(output truncated:"));
}

#[test]
fn commands_run_from_their_words() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(
        shell.eval("policy rr"),
        ok("scheduling policy: round-robin")
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert_eq!(
        shell.eval("policy(\"interactive\")"),
        ok("scheduling policy: interactive")
    );
    assert_eq!(
        shell.eval("ask hello world"),
        ok("Hello from the fake LLM.")
    );
    assert_eq!(
        host.asked.borrow().last().map(String::as_str),
        Some("hello world")
    );
    assert!(shell.eval("mem").text.contains("MiB free"));
    assert_eq!(shell.eval("cpus"), ok("4"));
    assert_eq!(shell.eval("cpus() + 1"), ok("5"));
    assert!(shell.eval("ps").text.contains("llm"));
    assert!(!shell.eval("mem 3").ok);
    assert!(!shell.eval("policy fastest").ok);
}

#[test]
fn a_variable_named_like_a_command_wins() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.eval("let mem = 3");
    assert_eq!(shell.eval("mem"), ok("3"));
}

#[test]
fn command_words_versus_rhai_syntax() {
    let none = |_: &str| false;
    assert_eq!(split_command("policy rr", none), Some(("policy", "rr")));
    assert_eq!(split_command("mem", none), Some(("mem", "")));
    assert_eq!(
        split_command("ask why is the sky blue", none),
        Some(("ask", "why is the sky blue"))
    );
    assert_eq!(split_command("policy(\"rr\")", none), None);
    assert_eq!(split_command("policy = 2", none), None);
    assert_eq!(split_command("ask (x)", none), None);
    assert_eq!(split_command("mem.len()", none), None);
    assert_eq!(split_command("let x = 1", none), None);
}

#[test]
fn many_lets_of_one_name_use_one_variable() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for i in 0..300 {
        assert!(shell.eval(&std::format!("let v = {i}")).ok, "let {i}");
    }
    assert_eq!(shell.sizes().0, 1);
    assert_eq!(shell.eval("v"), ok("299"));
}

#[test]
fn time_waiting_for_the_llm_is_not_charged() {
    let host = Rc::new(FakeHost {
        ask_takes_ns: 10_000_000_000,
        ..Rc::try_unwrap(FakeHost::new()).ok().unwrap()
    });
    let mut shell = shell(&host);
    let reply = shell.eval("let a = ask(\"q\"); let n = 0; for i in 0..3000 { n += 1 } a");
    assert_eq!(reply, ok("Hello from the fake LLM."));
}

#[test]
fn reset_and_syntax_errors() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.eval("let x = 1");
    shell.eval("fn f() { 2 }");
    assert_eq!(shell.eval("reset"), ok("variables and functions cleared"));
    assert!(!shell.eval("x").ok);
    assert!(!shell.eval("f()").ok);
    let reply = shell.eval("let = ;");
    assert!(!reply.ok && reply.text.starts_with("syntax error"));
    assert_eq!(
        reply.to_wire().split_once(' ').map(|(tag, _)| tag),
        Some("err")
    );
}
