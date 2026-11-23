//! The `goals` agent (ADR 0013 section 5; row G1 of `docs/plans/agents-os.md`):
//! the built-in agent whose routes carry badge 2. It runs `goalcore::runner` over
//! the goals the person approved. It serves nothing: routes form no cycle (ADR
//! 0001), so the shell never calls it, and everything goes one way, from here to
//! the shell over the badged route (#1047), and to llm.
//!
//! - **Goals come from the shell's queue** (A-core's G2, #1092). The shell runs
//!   `goal_set`, takes the person's one yes, sets the goal's grant ceiling and
//!   queues `goal-add <id>` with the goal's `kai-goals 1` file; the goal rows queue
//!   `goal-pause`, `goal-resume`, `goal-cancel` and `goal-list`. This agent takes
//!   each with `agent-step next` and says what came of it with `agent-step report
//!   <n> <result>`, the result in `goalcore::report`'s one shape (#1124): `ok` with
//!   the named goals' status lines, `gone <id>`, or `err <why>`.
//! - **The planner** asks llm for each goal's next step as an agent `act` job
//!   (`agent-start act`, then `agent-take`), with the goal, what is done and what
//!   is next as the line (S13-5's form). The call it answers is a step; `say` is
//!   the goal met. A `clarify` has no answer path yet (`goal_answer` comes later):
//!   the goal pauses with the question as what comes next, and the person's resume
//!   plans it again.
//! - **The steps** go to the shell as `agent-step goal=<id> <call>` (#1134). The
//!   shell admits them under that goal's ceiling only and decides the tier on the
//!   empty line: it runs what may run and puts any ask on the person's card. This
//!   agent then polls `agent-step wait <entry>` at most once a second until `ok ran
//!   #n`, `err not run #n` (a no) or `err unknown step #n` (a no). The person
//!   answers once, on the card; this agent answers no question itself.
//! - **A refusal at the ceiling** (`err refused`: the shell restarted and its
//!   grants are gone, or the goal was revoked) pauses the goal: it is not tried
//!   again until resumed.
//! - **A goal that ends** (met, given up) is told to the shell with `agent-step
//!   goal=<id> end`, which drops its ceiling.
//!
//! Goals are kept in RAM for now; `kai-goals 1` on the state volume is a
//! follow-up.
//!
//! `test=agent-job` is the `agent-job` boot case's client: an `ask` and an `act`
//! job on its own route, each answer taken, and a stranger's ticket refused.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use goalcore::report::Report;
use goalcore::runner::{Event, Outcome, Parts, Plan, Planner, Runner, Steps};
use goalcore::{AskKind, Door, Goal, Goals, Presence, State, Step, StepDone, Tier};
use llmproto::act::ActRequest;
use user::{arg, args, println, route, sys};

user::entry!(main);

/// How long one call to `llm` may take: an answer is taken later, so a call only
/// queues or reads.
const CALL_MS: u64 = 10_000;
/// How long the shell may take with a step, the person's card aside.
const STEP_MS: u64 = 120_000;
/// How long an agent-job answer is waited for in the boot case, and how often.
const ANSWER_MS: u64 = 300_000;
const POLL_MS: u64 = 200;
/// A turn: the shell's queue is read and the runner turns once, about once a
/// second (background class).
const TURN_MS: u64 = 1_000;
/// How often an asked step's answer is asked for: at most once a second.
const WAIT_POLL_MS: u64 = 1_000;
/// The routes, in the plan line's `uses=` order.
const LLM: usize = 0;
const SHELL: usize = 1;

fn main() -> i32 {
    let args = args();
    match arg(&args, "test") {
        Some("agent-job") => match agent_job() {
            Ok(()) => {
                println!("goals: agent-job ok");
                0
            }
            Err(why) => {
                println!("goals: agent-job failed: {why}");
                1
            }
        },
        _ => run(),
    }
}

/// The agent's loop: the shell's queued commands, then a turn of the runner.
fn run() -> i32 {
    println!("up; goals agent");
    let mut agent = Agent::default();
    let mut runner = Runner::new();
    loop {
        agent.take_queued();
        let now = now_ms();
        let mut steps = ShellSteps::default();
        let mut parts = Parts {
            planner: &mut agent.planner,
            door: &InCeiling,
            steps: &mut steps,
        };
        let events = runner.turn(&mut agent.goals, now, Presence::Here, &mut parts);
        // A goal the ceiling refused waits, untried, until it is resumed.
        for goal in steps.refused {
            if let Some(goal) = agent.goals.get_mut(goal)
                && goal.state == State::Active
            {
                let _ = goal.pause();
                println!("goals: goal {} paused: the shell refused its step", goal.id);
            }
        }
        for event in &events {
            report(event);
            match event {
                Event::Done { goal } | Event::Failed { goal } => end(*goal),
                Event::Waiting { goal, .. } => agent.hold_question(*goal),
                _ => {}
            }
        }
        sys::sleep_ms(TURN_MS);
    }
}

/// The goals, and the planner's llm jobs.
#[derive(Default)]
struct Agent {
    goals: Goals,
    planner: LlmPlanner,
}

impl Agent {
    /// The shell's queued commands, one call each, until it has none.
    fn take_queued(&mut self) {
        for _ in 0..goalcore::MAX_GOALS {
            let taken = match call_shell("agent-step next") {
                Ok(taken) => taken,
                Err(why) => {
                    println!("goals: the shell's queue: {why}");
                    return;
                }
            };
            let Some(queued) = taken.strip_prefix("ok ") else {
                println!("goals: the shell's queue: {taken}");
                return;
            };
            if queued.trim() == "none" {
                return;
            }
            // `#<n> <command>`, the goal-add file on the lines after it; the report
            // names the bare `<n>`.
            let (id, command) = queued.split_once(' ').unwrap_or((queued, ""));
            let id = id.trim_start_matches('#');
            let result = self.handle(command).encode();
            if let Err(why) = call_shell(&format!("agent-step report {id} {result}")) {
                println!("goals: report {id}: {why}");
            }
        }
    }

    /// One queued `goal-*` command, and what came of it.
    fn handle(&mut self, text: &str) -> Report {
        let (command, rest) = text.split_once([' ', '\n']).unwrap_or((text, ""));
        let now = now_ms();
        let id = rest.trim().parse::<u64>();
        match (command, id) {
            ("goal-add", _) => self.add(rest),
            ("goal-list", _) => Report::list(&self.goals),
            ("goal-pause", Ok(id)) => self.with(id, |goals| goals.get_mut(id)?.pause().ok()),
            ("goal-resume", Ok(id)) => self.with(id, |goals| goals.resume(id).ok()),
            ("goal-cancel", Ok(id)) => {
                self.planner.forget(id);
                self.with(id, |goals| goals.get_mut(id)?.cancel(now).ok())
            }
            ("goal-pause" | "goal-resume" | "goal-cancel", Err(_)) => {
                Report::Err(format!("{command} takes a goal id"))
            }
            _ => Report::Err(format!("unknown command {command:?}")),
        }
    }

    /// `goal-add <id>` and the `kai-goals 1` file of the one goal the person
    /// approved, kept under the shell's id.
    fn add(&mut self, rest: &str) -> Report {
        let Some((_, file)) = rest.split_once('\n') else {
            return Report::Err(String::from("goal-add takes the goal's file"));
        };
        let file = match Goals::parse(file) {
            Ok(file) => file,
            Err(why) => return Report::Err(format!("not a kai-goals 1 file: {why}")),
        };
        let [goal] = file.list() else {
            return Report::Err(String::from("goal-add takes one goal"));
        };
        let id = goal.id;
        match self.goals.adopt(goal.clone()) {
            Ok(()) => self.status(id),
            Err(why) => Report::Err(why.words()),
        }
    }

    /// Changes goal `id` with `change`: its status after, `gone` when there is no
    /// such goal, or why the change was refused.
    fn with(&mut self, id: u64, change: impl FnOnce(&mut Goals) -> Option<()>) -> Report {
        if self.goals.get(id).is_none() {
            return Report::Gone(id);
        }
        let before = self.goals.get(id).map(|goal| goal.state);
        match change(&mut self.goals) {
            Some(()) => self.status(id),
            None => Report::Err(format!(
                "goal {id} is {}; that does not apply",
                before.map_or("gone", State::word)
            )),
        }
    }

    fn status(&self, id: u64) -> Report {
        match self.goals.get(id) {
            Some(goal) => Report::of(goal),
            None => Report::Gone(id),
        }
    }

    /// A goal waiting on the planner's question has no answer path yet: it pauses,
    /// with the question as what comes next, and the person's resume plans it again.
    fn hold_question(&mut self, id: u64) {
        let Some(goal) = self.goals.get_mut(id) else {
            return;
        };
        let question = match &goal.waiting {
            Some(ask) if ask.kind == AskKind::Question => ask.why.clone(),
            _ => return,
        };
        if goal.answered().is_ok() {
            let planned = goal.planned;
            goal.plan(planned, &format!("waits for your answer: {question}"));
            let _ = goal.pause();
            println!("goals: goal {id} paused with a question: {question}");
        }
    }
}

/// Tells the shell that goal `goal` ended, so it drops the goal's ceiling.
fn end(goal: u64) {
    match call_shell(&format!("agent-step goal={goal} end")) {
        Ok(reply) => println!("goals: goal {goal} end: {reply}"),
        Err(why) => println!("goals: goal {goal} end: {why}"),
    }
}

/// The goals agent's door: inside the ceiling the person approved, every step goes
/// to the shell, whose own door decides its tier on the empty line and asks the
/// person on the card. So goalcore never asks on its own, and the person answers
/// once.
struct InCeiling;

impl Door<String> for InCeiling {
    fn tier(&self, _step: &Step<String>) -> Tier {
        Tier::Run
    }
}

/// Each goal's next step, from llm's agent `act` jobs: one job a goal at a time.
#[derive(Default)]
struct LlmPlanner {
    /// The ticket of each goal's job in flight.
    jobs: Vec<(u64, String)>,
}

impl LlmPlanner {
    fn forget(&mut self, goal: u64) {
        self.jobs.retain(|(g, _)| *g != goal);
    }
}

impl Planner<String> for LlmPlanner {
    fn next(&mut self, goal: &Goal, _now: u64) -> (Plan<String>, u64) {
        let Some(at) = self.jobs.iter().position(|(g, _)| *g == goal.id) else {
            match start_plan(goal) {
                Ok(ticket) => self.jobs.push((goal.id, ticket)),
                Err(why) => println!("goals: goal {} not planned: {why}", goal.id),
            }
            return (Plan::Later, 0);
        };
        let taken = match call_llm(&format!("agent-take {}", self.jobs[at].1)) {
            Ok(taken) => taken,
            Err(why) => return (Plan::Fail(why), 0),
        };
        if taken == "pending" {
            return (Plan::Later, 0);
        }
        self.jobs.remove(at);
        (plan_of(&taken), 0)
    }
}

/// Starts the llm job that plans `goal`'s next step: its ticket.
fn start_plan(goal: &Goal) -> Result<String, String> {
    let done: Vec<&str> = goal
        .log
        .iter()
        .rev()
        .take(4)
        .map(|step| step.proposal.row.as_str())
        .collect();
    let done = match done.is_empty() {
        true => String::from("nothing yet"),
        false => done.join(", "),
    };
    let next = match goal.next.is_empty() {
        true => "the first step",
        false => goal.next.as_str(),
    };
    let request = ActRequest {
        line: format!("goal: {}\ndone: {done}\nnext: {next}", goal.text),
        state: String::new(),
        timeout_ms: STEP_MS,
        read: None,
        apps: String::new(),
    };
    let encoded = request.encode();
    let body = encoded
        .strip_prefix("act ")
        .ok_or("an act request without act")?;
    let started = call_llm(&format!("agent-start act {body}"))?;
    started
        .strip_prefix("ok ")
        .map(|ticket| ticket.trim().to_string())
        .ok_or(started)
}

/// What llm's answer to a planning job says the goal does next.
fn plan_of(taken: &str) -> Plan<String> {
    let Some(answer) = taken.strip_prefix("ok ") else {
        return Plan::Fail(format!("the planner failed: {taken}"));
    };
    let call = match llmproto::act::decode_answer(answer) {
        Ok(call) => call,
        Err(why) => return Plan::Fail(format!("the planner gave no call: {why:?}")),
    };
    let text = |key: &str| {
        call.arguments
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    match call.name.as_str() {
        "say" => Plan::Done(text("text")),
        "clarify" => Plan::Clarify(text("question")),
        _ => Plan::Step(Step {
            targets: targets(&call),
            row: call.name.clone(),
            tokens_est: 0,
            call: call.text(),
        }),
    }
}

/// Every path a file row's call acts on, from the home folder, for goalcore's scope
/// check: the row's target arguments.
fn targets(call: &llmproto::act::Call) -> Vec<String> {
    let Some(row) = agentabi::find(&agentabi::ROWS, &call.name) else {
        return Vec::new();
    };
    if !row.name.starts_with("fs_") {
        return Vec::new();
    }
    row.args
        .iter()
        .filter(|arg| matches!(arg.role, agentabi::Role::Target))
        .filter_map(|arg| call.arguments.get(arg.name)?.as_str())
        .map(|path| path.strip_prefix("/home/user/").unwrap_or(path).to_string())
        .collect()
}

/// The shell's door for this agent's steps, and the goals its ceiling refused.
#[derive(Default)]
struct ShellSteps {
    refused: Vec<u64>,
}

impl Steps<String> for ShellSteps {
    fn run(&mut self, goal: u64, step: &Step<String>) -> Outcome {
        let reply = match call_shell(&format!("agent-step goal={goal} {}", step.call)) {
            Ok(reply) => reply,
            Err(why) => return Outcome::Done(failed(None, why)),
        };
        // `ok asked <entry>: on the person's card`: the answer comes by `wait`.
        let reply = match reply.strip_prefix("ok asked ") {
            Some(rest) => {
                let id = rest.split(':').next().unwrap_or("").trim().to_string();
                match wait_for(&id) {
                    Ok(answer) => answer,
                    Err(why) => return Outcome::Done(failed(None, why)),
                }
            }
            None => reply,
        };
        // The person's no on the card: the step did not run.
        if let Some(why) = reply.strip_prefix("err not run ") {
            return Outcome::Denied(why.to_string());
        }
        // Refused at the ceiling: the shell restarted (its grants are not kept) or
        // the goal was revoked. The goal pauses; it is not tried again in a loop.
        if reply.starts_with("err refused") {
            self.refused.push(goal);
        }
        Outcome::Done(step_done(&reply))
    }
}

/// Polls `agent-step wait <entry>`, at most once a second, until the person
/// answered on the card. An entry the shell does not know is a no.
fn wait_for(id: &str) -> Result<String, String> {
    loop {
        let reply = call_shell(&format!("agent-step wait {id}"))?;
        if reply.starts_with("err unknown step") {
            return Ok(format!("err not run #{id}: no answer is kept for it"));
        }
        if !reply.starts_with("ok waiting") {
            return Ok(reply);
        }
        sys::sleep_ms(WAIT_POLL_MS);
    }
}

/// What the shell's reply to a step says: `ok <tier> #<entry>: …` or `ok ran
/// #<entry>: …` ran, anything else failed; the entry is the actlog's.
fn step_done(reply: &str) -> StepDone {
    let (ok, rest) = match reply.split_once(' ') {
        Some(("ok", rest)) => (true, rest),
        Some((_, rest)) => (false, rest),
        None => (false, reply),
    };
    let entry = rest
        .split_once('#')
        .and_then(|(_, after)| after.split(':').next()?.trim().parse::<u64>().ok());
    match ok {
        true => StepDone {
            entry,
            tokens: 0,
            failed: None,
        },
        false => failed(entry, rest.to_string()),
    }
}

fn failed(entry: Option<u64>, why: String) -> StepDone {
    StepDone {
        entry,
        tokens: 0,
        failed: Some(why),
    }
}

/// One event, as the log says it; the panel's lines come with G3.
fn report(event: &Event) {
    match event {
        Event::Ran { goal, step, entry } => {
            println!("goals: goal {goal} step {step} ran (entry {entry:?})");
        }
        Event::Denied { goal, step } => {
            println!("goals: goal {goal} step {step}: the person said no");
        }
        Event::Repeated { goal, row } => {
            println!("goals: goal {goal} paused: {row} again, after the person said no to it");
        }
        Event::Waiting { line, .. } => println!("goals: {line}"),
        Event::Paused { goal, pause } => println!("goals: goal {goal} paused ({pause:?})"),
        Event::Done { goal } => println!("goals: goal {goal} done"),
        Event::Failed { goal } => println!("goals: goal {goal} failed"),
        Event::CheckIn { check, .. } => println!("goals: {}", check.line),
    }
}

/// The time goals count in, in ms: the host's clock when the firmware gave one, so
/// a deadline is a wall-clock time; else the boot's.
fn now_ms() -> u64 {
    match sys::clock_now() {
        Ok(secs) => secs.saturating_mul(1000),
        Err(_) => sys::time_ns() / 1_000_000,
    }
}

/// One request to the shell on this agent's badged route, and its reply.
fn call_shell(text: &str) -> Result<String, String> {
    let shell = route(SHELL).ok_or("no route to the shell (uses=llm+shell)")?;
    let mut reply = alloc::vec![0u8; kabi::MAX_MESSAGE];
    let len = sys::call_route(shell, text.as_bytes(), &mut reply, STEP_MS)
        .map_err(|error| format!("shell: {error:?}"))?;
    Ok(String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned())
}

/// One request to `llm` on this agent's route, and its reply.
fn call_llm(text: &str) -> Result<String, String> {
    let llm = route(LLM).ok_or("no route to llm (uses=llm)")?;
    let mut reply = alloc::vec![0u8; kabi::MAX_MESSAGE];
    let len = sys::call_route(llm, text.as_bytes(), &mut reply, CALL_MS)
        .map_err(|error| format!("llm: {error:?}"))?;
    Ok(String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned())
}

/// Starts a job of `kind` and waits for its answer (the boot case's client).
fn job(kind: &str, request: &str) -> Result<String, String> {
    let started = call_llm(&format!("agent-start {kind} {request}"))?;
    let ticket: String = started
        .strip_prefix("ok ")
        .ok_or_else(|| format!("agent-start {kind}: {started}"))?
        .trim()
        .into();
    println!("goals: {kind} job {ticket} queued");
    let began = sys::time_ns();
    loop {
        let taken = call_llm(&format!("agent-take {ticket}"))?;
        if taken != "pending" {
            let ms = (sys::time_ns() - began) / 1_000_000;
            println!("goals: {kind} job {ticket} answered in {ms} ms");
            return Ok(taken);
        }
        if sys::time_ns() - began > ANSWER_MS * 1_000_000 {
            return Err(format!(
                "{kind} job {ticket} not answered in {ANSWER_MS} ms"
            ));
        }
        sys::sleep_ms(POLL_MS);
    }
}

fn agent_job() -> Result<(), String> {
    let asked = job("ask", "what day comes after monday?")?;
    if !asked.starts_with("ok ") {
        return Err(format!("ask: {asked}"));
    }
    println!("goals: ask answer {}", asked.len());
    let request = ActRequest {
        // A line the mock server answers with a call (its `ACTS` table).
        line: String::from("how many processors do I have"),
        state: String::new(),
        timeout_ms: 120_000,
        read: None,
        apps: String::new(),
    };
    // `agent-start <kind> <request>` carries the request's own text without its
    // command word, which llm puts back from the kind.
    let encoded = request.encode();
    let body = encoded
        .strip_prefix("act ")
        .ok_or("an act request without act")?;
    let acted = job("act", body)?;
    if !acted.starts_with("ok ") {
        return Err(format!("act: {acted}"));
    }
    println!("goals: act answer {}", acted.len());
    // A ticket this agent never had is one that never was.
    let unknown = call_llm("agent-take 999999")?;
    if unknown != "ended unknown ticket" {
        return Err(format!("a stranger's ticket read {unknown:?}"));
    }
    println!("goals: unknown ticket refused");
    Ok(())
}
