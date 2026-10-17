extern crate std;

use super::*;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use kabi::{Metrics, policy};

/// A host whose clock moves `step_ns` on every reading.
struct FakeHost {
    clock: Cell<u64>,
    step_ns: Cell<u64>,
    /// Interrupt after this many polls.
    interrupt_in: Cell<Option<u32>>,
    heap: Cell<u64>,
    policy: Cell<Option<u64>>,
    asked: RefCell<Vec<String>>,
    ask_takes_ns: u64,
}

impl FakeHost {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            clock: Cell::new(0),
            step_ns: Cell::new(1000),
            interrupt_in: Cell::new(None),
            heap: Cell::new(0),
            policy: Cell::new(None),
            asked: RefCell::new(Vec::new()),
            ask_takes_ns: 0,
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
        m.policy = self.policy.get().unwrap_or(policy::INTERACTIVE);
        m.running_policy = m.policy;
        Ok(m)
    }

    fn set_policy(&self, id: u64) -> Result<(), kabi::Error> {
        self.policy.set(Some(id));
        Ok(())
    }

    fn ask(&self, prompt: &str) -> Result<String, String> {
        self.asked.borrow_mut().push(String::from(prompt));
        self.clock.set(self.clock.get() + self.ask_takes_ns);
        Ok(String::from("Hello from the fake LLM."))
    }
}

fn shell(host: &Rc<FakeHost>) -> Shell {
    Shell::new(Rc::clone(host) as Rc<dyn Host>, DEFAULT_CONFIG)
}

fn ok(text: &str) -> Reply {
    Reply {
        ok: true,
        text: String::from(text),
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
    host.heap.set(DEFAULT_CONFIG.heap_budget + 1);
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
