//! One transaction per script (ADR 0002 D5): the analyser's property that every
//! live call is in the static plan, the live re-check, rollback on a limit, and the
//! outbox.

use super::*;
use crate::analyse::Plan;
use crate::script::{self, Queued};
use alloc::boxed::Box;
use alloc::format;
use alloc::vec;

/// What the live re-check says when a call is not in the plan.
const NOT_IN_PLAN: &str = "is not a call the checked script makes";

fn answer(script: &str) -> Answer {
    Answer {
        script: String::from(script),
        summary: String::from("Does it."),
    }
}

/// xorshift64*, so the corpus is the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Writes random strict-profile scripts over the script API: `let`, assignment,
/// `if`, `for`, `switch`, `try`, early exits, method calls and interpolation.
struct Gen {
    rng: Rng,
    vars: Vec<String>,
    count: usize,
}

impl Gen {
    fn script(&mut self) -> String {
        self.vars.clear();
        self.block(0)
    }

    fn block(&mut self, depth: usize) -> String {
        let scope = self.vars.len();
        let mut out = String::new();
        for _ in 0..1 + self.rng.below(4) {
            out.push_str(&self.stmt(depth));
            out.push('\n');
        }
        self.vars.truncate(scope);
        out
    }

    fn stmt(&mut self, depth: usize) -> String {
        let nested = depth < 3;
        match self.rng.below(if nested { 11 } else { 6 }) {
            0 | 1 => {
                let value = self.expr();
                self.count += 1;
                let name = format!("v{}", self.count);
                self.vars.push(name.clone());
                format!("let {name} = {value};")
            }
            2 => format!("set_policy({});", self.policy()),
            3 => format!("{}.set_policy();", self.policy()),
            4 => match self.vars.len() {
                0 => String::from("print(cpus());"),
                n => {
                    let name = self.vars[self.rng.below(n)].clone();
                    format!("{name} = {};", self.expr())
                }
            },
            5 => format!("print(`${{get_policy()}} ${{{}}}`);", self.expr()),
            6 => format!(
                "if {} {{\n{}}} else {{\n{}}}",
                self.cond(),
                self.block(depth + 1),
                self.block(depth + 1)
            ),
            7 => {
                let over = ["0..3", "processes()", "[\"rr\", \"interactive\"]"][self.rng.below(3)];
                self.count += 1;
                let name = format!("v{}", self.count);
                self.vars.push(name.clone());
                let body = self.block(depth + 1);
                self.vars.pop();
                format!("for {name} in {over} {{\n{body}}}")
            }
            8 => format!(
                "switch cpus() {{ 2 => set_policy({}), 4 => print({}), _ => () }}",
                self.policy(),
                self.expr()
            ),
            9 => format!(
                "try {{\n{}}} catch {{\n{}}}",
                self.block(depth + 1),
                self.block(depth + 1)
            ),
            _ => format!("if {} {{ return; }}", self.cond()),
        }
    }

    fn policy(&mut self) -> String {
        match self.rng.below(5) {
            0 => String::from("\"rr\""),
            1 => String::from("\"interactive\""),
            2 => String::from("get_policy()"),
            3 => format!("if {} {{ \"rr\" }} else {{ get_policy() }}", self.cond()),
            _ => self
                .var()
                .unwrap_or_else(|| String::from("\"round-robin\"")),
        }
    }

    fn expr(&mut self) -> String {
        match self.rng.below(6) {
            0 => self.policy(),
            1 => String::from("cpus()"),
            2 => format!("[{}, {}][0]", self.policy(), self.policy()),
            3 => String::from("uptime_ms() % 7"),
            4 => format!("{} + 1", self.rng.below(9)),
            _ => self
                .var()
                .unwrap_or_else(|| String::from("memory().free_mib")),
        }
    }

    fn cond(&mut self) -> String {
        match self.rng.below(4) {
            0 => String::from("cpus() > 2"),
            1 => format!("get_policy() == {}", self.policy()),
            2 => String::from("true"),
            _ => String::from("processes().len() < 16"),
        }
    }

    fn var(&mut self) -> Option<String> {
        match self.vars.len() {
            0 => None,
            n => Some(self.vars[self.rng.below(n)].clone()),
        }
    }
}

#[test]
fn every_live_call_is_in_the_static_plan() {
    let mut generate = Gen {
        rng: Rng(0x5eed_0fa1),
        vars: Vec::new(),
        count: 0,
    };
    let (mut checked_ok, mut ran_ok) = (0, 0);
    for _ in 0..2000 {
        let text = generate.script();
        let host = FakeHost::new();
        let dyn_host: Rc<dyn Host> = host.clone();
        let checked = match script::check(&answer(&text), &dyn_host, &DEFAULT_CONFIG) {
            Ok(checked) => checked,
            Err(refusal) => {
                assert!(
                    !refusal.to_string().contains(NOT_IN_PLAN),
                    "trial: {refusal}\n{text}"
                );
                continue;
            }
        };
        checked_ok += 1;
        let ran = script::run(&checked, &dyn_host, &DEFAULT_CONFIG);
        assert!(!ran.text.contains(NOT_IN_PLAN), "run: {}\n{text}", ran.text);
        ran_ok += usize::from(ran.ok);
    }
    // The corpus is worth something only if most of it gets through.
    assert!(
        checked_ok > 1000 && ran_ok > 800,
        "{checked_ok} checked, {ran_ok} ran"
    );
}

#[test]
fn a_live_call_the_plan_does_not_have_is_refused_and_rolled_back() {
    let host = FakeHost::new();
    let dyn_host: Rc<dyn Host> = host.clone();
    let mut checked = script::check(
        &answer("set_policy(\"rr\");\nset_policy(\"interactive\");"),
        &dyn_host,
        &DEFAULT_CONFIG,
    )
    .unwrap();
    // As if the analyser had seen only the first call.
    let mut plan: Plan = (*checked.plan).clone();
    plan.sites.truncate(1);
    checked.plan = Rc::new(plan);
    let ran = script::run(&checked, &dyn_host, &DEFAULT_CONFIG);
    assert!(!ran.ok);
    assert!(
        ran.text
            .contains("set_policy(interactive) is not a call the checked script makes"),
        "{}",
        ran.text
    );
    // The first switch is in the journal, for the caller to put back.
    assert_eq!(ran.done.len(), 1);
}

#[test]
fn a_limit_after_a_change_leaves_nothing_changed() {
    // The trial's stand-in has two CPUs, so only the real run spins.
    let text = "set_policy(\"rr\");\nlet n = 0;\nif cpus() > 2 { for i in 0..1000000 { n += i; } }";
    let (host, mut shell) = super::scripts::with_answers(vec![Ok(answer(text))]);
    // Every clock reading is 100 ms later, so the spin hits the time limit.
    host.step_ns.set(100_000_000);
    let reply = shell.eval("switch to round robin");
    assert_eq!(
        reply,
        Reply::err("Nothing changed: stopped: time limit (2000 ms) reached")
    );
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn the_outbox_sends_in_order_and_stops_at_a_failure() {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let queue = |action: &'static str, ok: bool| {
        let sent = Rc::clone(&sent);
        Queued {
            action,
            send: Box::new(move || {
                sent.borrow_mut().push(action);
                if ok {
                    Ok(())
                } else {
                    Err(String::from("refused"))
                }
            }),
        }
    };
    let mut text = String::new();
    assert!(script::flush(
        vec![queue("a", true), queue("b", true)],
        &mut text
    ));
    let failed = script::flush(vec![queue("c", false), queue("d", true)], &mut text);
    assert!(!failed);
    assert_eq!(*sent.borrow(), ["a", "b", "c"]);
    assert_eq!(text, "c: not sent: refused");
}
