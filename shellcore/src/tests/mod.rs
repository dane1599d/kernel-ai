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

/// What the fake `cc_trial` answers past its queue.
/// The SHA-256 of the artifact the fake `cc` wrote.
const ARTIFACT_SHA: &str = "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5";
const TRIAL_REPORT: &str = "init, started, a key and a click ran; 1 window";

mod act;
mod actions;
mod agentconsent;
mod agentstep;
mod apart;
mod appbuild;
mod appcard;
mod appgrants;
mod appinstall;
mod appjob;
mod appline;
mod apps;
mod ccbuild;
mod centry;
mod cmode;
mod cprogram;
mod deskcard;
mod door;
mod door_build;
mod effects;
mod fake_host;
mod files;
mod fuzz;
mod goalreports;
mod goals;
mod goalsteps;
mod grantleaks;
mod grantundo;
mod guard;
mod log;
mod logdisk;
mod look;
mod models;
mod models_window;
mod more;
mod netask;
mod netgrant;
mod netrows;
mod outbox;
mod packages;
mod panel;
mod power;
mod profiles;
mod scripts;
mod sendrow;
mod settings;
mod standing;
mod told;
mod transaction;
mod tune;
mod ui;
mod undo_rows;
mod waiting;

/// A host whose clock moves `step_ns` on every reading.
struct FakeHost {
    clock: Cell<u64>,
    step_ns: Cell<u64>,
    /// Interrupt after this many polls.
    interrupt_in: Cell<Option<u32>>,
    /// A Stop pressed for the whole request, as the kernel keeps it (#887).
    stopped: Cell<bool>,
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
    act_apps: RefCell<Vec<String>>,
    /// What the apps ask while each `act` is in flight: handed to its `serve`.
    serving: RefCell<Option<alloc::boxed::Box<Serving>>>,
    /// What `unix_seconds` gives.
    wall_clock: Cell<Option<u64>>,
    /// Installed packages' `kai-actions 1` answers, as `pkg` reads them from
    /// the store, and each package asked for them.
    installed: RefCell<Vec<(String, String)>>,
    installed_asked: RefCell<Vec<String>>,
    /// When set, `installed_rows` fails with this, as a `pkg` that did not
    /// answer.
    installed_down: RefCell<Option<String>>,
    /// What the shell asked `desktop`, and how `desktop` answers.
    ui_calls: RefCell<Vec<crate::ui::UiCall>>,
    /// The hosts the network rows reached, as `row host`.
    net_seen: RefCell<Vec<String>>,
    /// The questions `query` asked, and the model's answer to each.
    queries: RefCell<Vec<llmproto::query::QueryRequest>>,
    query_answer: RefCell<Result<String, String>>,
    ui_answer: RefCell<Option<UiAnswer>>,
    state: RefCell<crate::state::State>,
    /// Turns sent to the conversation memory, and how often it was forgotten.
    remembered: RefCell<Vec<(String, String, String)>>,
    forgot: Cell<u32>,
    audit: RefCell<Vec<String>>,
    /// What the supervisor was told to keep running, and the saved list.
    kept: RefCell<Vec<(String, bool)>>,
    keep_file: RefCell<Option<String>>,
    /// How many `keep` requests the supervisor refuses next, as one for an app
    /// not serving yet.
    keep_fails: Cell<u32>,
    /// A refusal the supervisor gives every `keep` request, as for an app it
    /// does not run.
    keep_refusal: RefCell<Option<String>>,
    /// The saved restart window (`kai-updates 1`).
    restart_file: RefCell<Option<String>>,
    /// The installed apps' rows, and what each `app_call` sent and answers.
    apps: RefCell<crate::apps::Apps>,
    app_requests: RefCell<Vec<(String, String)>>,
    app_replies: RefCell<VecDeque<String>>,
    /// Services the supervisor answers "no such app" for (the shell, llm).
    not_apps: RefCell<Vec<String>>,
    /// Errors the next app calls answer, before any reply: as the shell's
    /// `ask_app` gives the supervisor's `err ...` answers.
    app_errors: RefCell<VecDeque<String>>,
    /// The saved appearance (`kai-appearance 1`).
    appearance: RefCell<Option<String>>,
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
    /// The `cc` service, once a test starts one (`c>`).
    cc: RefCell<Option<ccserve::Service<ccserve::rows::Rows>>>,
    /// Replies a test scripts for cc, given before the service answers, and
    /// every request the shell sent cc.
    cc_script: RefCell<VecDeque<Vec<u8>>>,
    cc_sent: RefCell<Vec<String>>,
    /// `cc build`'s calls to `pkg` and the host, in order, the answers a
    /// test scripts for `pkg`, and what `cc_build` gives.
    pkg_sent: RefCell<Vec<String>>,
    pkg_answers: RefCell<VecDeque<Result<String, String>>>,
    cc_built: RefCell<Option<Result<crate::ccbuild::CcBuilt, String>>>,
    /// Builds `cc_build` answers in turn, before `cc_built` (`app_build`'s
    /// repairs).
    pub cc_builds: RefCell<VecDeque<Result<crate::ccbuild::CcBuilt, String>>>,
    /// Whether `script_start` asks off the line, as the OS's shell does.
    pub background: Cell<bool>,
    /// The ticket of the answer `script_start` is waiting for, and the last.
    pub in_flight: Cell<Option<u64>>,
    pub tickets: Cell<u64>,
    /// The tickets `script_stop` was given. The model may still be answering.
    pub stops: RefCell<Vec<u64>>,
    /// Each `model_warm` hint, in order.
    pub warms: RefCell<Vec<llmproto::script::Lang>>,
    /// Set to let `script_take` hand over the answer in flight.
    pub answered: Cell<bool>,
    /// `cc_trial`'s answers in turn; past them, the trial passes.
    /// What `cc_artifact` gives for the app built last.
    pub cc_artifact: RefCell<Option<crate::ccbuild::Artifact>>,
    pub cc_trials: RefCell<VecDeque<Result<String, String>>>,
    /// Each app's source `app_source` wrote, in order.
    pub app_sources: RefCell<Vec<(String, String)>>,
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
    pub withheld_disk: RefCell<Option<String>>,
    pub net_disk: RefCell<Option<String>>,
    /// `/system/state/standing`, and the event source and owners standing
    /// instructions run through, when `triggers_on` is set.
    standing_disk: RefCell<Option<String>>,
    triggers_on: Cell<bool>,
    triggers: FakeTriggers,
    /// What the supervisor set each process to (class, page quota); a process
    /// not here runs normal with 4,096 pages. And the app with focus.
    profile_state: RefCell<std::collections::BTreeMap<String, (kaiprofile::Class, u64)>>,
    focused: RefCell<Option<String>>,
    /// What `updated` holds for the model rows, and each request it was sent.
    models: RefCell<comp_models::Models>,
    model_requests: RefCell<Vec<String>>,
    /// What `updated` answers every request with instead, when set.
    model_answer: RefCell<Option<String>>,
    /// Each restart, with the log on the disk when it came (backlog item 13).
    reboots: RefCell<Vec<Option<String>>>,
    /// Every send the network was given (`net_send`), with the log on disk
    /// as it was then, and what the next sends answer (else `Ok`).
    sends: RefCell<Vec<(crate::outbox::Send, Option<String>)>>,
    send_answers: RefCell<VecDeque<Result<String, crate::outbox::SendFailed>>>,
    /// Set to make the next log writes fail.
    log_fails: Cell<u32>,
}

/// What standing instructions asked the desktop and the owners for.
#[derive(Default)]
pub(super) struct FakeTriggers {
    pub watched: RefCell<Vec<crate::standing::When>>,
    pub runs: RefCell<Vec<(u64, String, String, bool)>>,
    /// The owner's answer: `Err` when it finds no live rule covering a firing.
    pub refuse: RefCell<Option<String>>,
    /// The event box's lines waiting to be read.
    pub waiting: RefCell<String>,
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

    fn waiting(&self) -> String {
        core::mem::take(&mut *self.waiting.borrow_mut())
    }
}

type Files = std::collections::BTreeMap<String, String>;

/// What a test's apps ask while `act` is in flight, given `act_serving`'s
/// `serve`.
type Serving =
    dyn Fn(&mut dyn FnMut(crate::waiting::Waiting<'_>) -> Option<crate::waiting::Served>);

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
            stopped: Cell::new(false),
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
            act_apps: RefCell::new(Vec::new()),
            serving: RefCell::new(None),
            wall_clock: Cell::new(None),
            installed: RefCell::new(Vec::new()),
            installed_asked: RefCell::new(Vec::new()),
            installed_down: RefCell::new(None),
            ui_calls: RefCell::new(Vec::new()),
            net_seen: RefCell::new(Vec::new()),
            queries: RefCell::new(Vec::new()),
            query_answer: RefCell::new(Ok(String::from("yes"))),
            ui_answer: RefCell::new(None),
            state: RefCell::new(crate::state::State::default()),
            remembered: RefCell::new(Vec::new()),
            forgot: Cell::new(0),
            audit: RefCell::new(Vec::new()),
            kept: RefCell::new(Vec::new()),
            keep_file: RefCell::new(None),
            keep_fails: Cell::new(0),
            keep_refusal: RefCell::new(None),
            restart_file: RefCell::new(None),
            apps: RefCell::new(crate::apps::Apps::default()),
            app_requests: RefCell::new(Vec::new()),
            app_replies: RefCell::new(VecDeque::new()),
            not_apps: RefCell::new(Vec::new()),
            app_errors: RefCell::new(VecDeque::new()),
            appearance: RefCell::new(None),
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
            cc: RefCell::new(None),
            cc_script: RefCell::new(VecDeque::new()),
            cc_sent: RefCell::new(Vec::new()),
            pkg_sent: RefCell::new(Vec::new()),
            pkg_answers: RefCell::new(VecDeque::new()),
            cc_built: RefCell::new(None),
            cc_builds: RefCell::new(VecDeque::new()),
            cc_trials: RefCell::new(VecDeque::new()),
            cc_artifact: RefCell::new(Some(crate::ccbuild::Artifact {
                sha256: String::from(ARTIFACT_SHA),
                imports: alloc::vec![
                    String::from("kai:app/log@0.1.0"),
                    String::from("kai:ui/view@0.1.0"),
                ],
            })),
            background: Cell::new(false),
            in_flight: Cell::new(None),
            tickets: Cell::new(0),
            answered: Cell::new(false),
            stops: RefCell::new(Vec::new()),
            warms: RefCell::new(Vec::new()),
            app_sources: RefCell::new(Vec::new()),
            files: RefCell::new(Files::new()),
            txns: RefCell::new(Vec::new()),
            one_action: Cell::new(false),
            open_txn: Cell::new(None),
            log_disk: RefCell::new(None),
            log_writes: Cell::new(0),
            rules_disk: RefCell::new(None),
            withheld_disk: RefCell::new(None),
            net_disk: RefCell::new(None),
            standing_disk: RefCell::new(None),
            triggers_on: Cell::new(false),
            triggers: FakeTriggers::default(),
            profile_state: RefCell::new(std::collections::BTreeMap::new()),
            focused: RefCell::new(None),
            models: RefCell::new(models::store()),
            model_requests: RefCell::new(Vec::new()),
            model_answer: RefCell::new(None),
            reboots: RefCell::new(Vec::new()),
            sends: RefCell::new(Vec::new()),
            send_answers: RefCell::new(VecDeque::new()),
            log_fails: Cell::new(0),
        })
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
