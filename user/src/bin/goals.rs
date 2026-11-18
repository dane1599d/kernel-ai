//! The `goals` agent (ADR 0013 section 5; row G1 of `docs/plans/agents-os.md`):
//! the built-in agent whose routes carry badge 2. It runs `goalcore::runner` over
//! the goals the person approved, with three parts:
//!
//! - **Goals come from the shell**, which runs `goal_set`, gets the person's one
//!   yes and sets the goal's grant ceiling (`agentid::Roster::grant_goal`), then
//!   hands the goal over: `goal-add <kai-goals 1 file with that goal>`,
//!   `goal-pause <id>`, `goal-resume <id>`, `goal-cancel <id>`, `goal-list`. They
//!   are served only to `kai`'s badge, on the shell's route to goals.
//! - **The planner** asks llm for each goal's next step as an agent `act` job
//!   (`agent-start act`, then `agent-take`), with the goal, what is done and what
//!   is next as the line (S13-5's form). The call it answers is a step; `say` is
//!   the goal met, `clarify` a question.
//! - **The steps** go to the shell as `agent-step goal=<id> <call>`. The shell
//!   admits them under that goal's ceiling and decides the tier on the empty line:
//!   it runs what may run and puts any ask on the person's card. Then this agent
//!   waits, and polls `agent-step wait <id>`. The person answers once, on the
//!   card. This agent holds no action handle and answers no question itself.
//!
//! It never blocks on the shell: a call to the shell goes out with `CALL_SUBMIT`
//! and is polled while this agent goes on serving the shell's `goal-*` requests,
//! so the two can call each other. Goals are kept in RAM for now; `kai-goals 1`
//! on the state volume is a follow-up.
//!
//! `test=agent-job` is the `agent-job` boot case's client: an `ask` and an `act`
//! job on its own route, each answer taken, and a stranger's ticket refused.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use goalcore::runner::{Event, Parts, Plan, Planner, Runner, Steps};
use goalcore::{Door, Goal, Goals, Presence, Step, StepDone, Tier};
use kabi::handle::{Handle, RecvInfo};
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
/// A turn of the runner, and how long a wait for a request lasts between turns.
const TURN_MS: u64 = 1_000;
/// How often an asked step's answer is asked for.
const WAIT_POLL_MS: u64 = 1_000;
/// `kai`'s badge: only the shell hands goals over.
const KAI: u64 = 1;
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

/// The agent's loop: serve the shell's `goal-*` requests, and take a turn of the
/// runner about once a second.
fn run() -> i32 {
    println!("up; goals agent");
    let mut agent = Agent::default();
    let mut runner = Runner::new();
    let mut request = alloc::vec![0u8; kabi::MAX_MESSAGE];
    loop {
        agent.serve_once(&mut request, TURN_MS);
        let now = now_ms();
        let mut steps = ShellSteps {
            held: &mut agent.held,
        };
        let mut parts = Parts {
            planner: &mut agent.planner,
            door: &InCeiling,
            steps: &mut steps,
        };
        let events = runner.turn(&mut agent.goals, now, Presence::Here, &mut parts);
        agent.apply_held();
        for event in events {
            report(&event);
        }
    }
}

/// The goals, the planner's llm jobs, and the shell's requests that came while a
/// step was out: the goal changes they ask for are applied after the turn, so a
/// goal never changes under a step in flight.
#[derive(Default)]
struct Agent {
    goals: Goals,
    planner: LlmPlanner,
    held: Vec<String>,
}

impl Agent {
    /// Waits up to `wait_ms` for one request and answers it.
    fn serve_once(&mut self, request: &mut [u8], wait_ms: u64) {
        let mut info = RecvInfo::default();
        let Ok(len) = recv(request, wait_ms, &mut info) else {
            return;
        };
        let text = String::from_utf8_lossy(&request[..len.min(request.len())]).into_owned();
        let reply = match info.badge == KAI {
            true => self.handle(&text),
            false => String::from("err only the shell hands goals over"),
        };
        if let Err(error) = sys::svc_reply(reply.as_bytes()) {
            println!("goals: svc_reply failed: {error:?}");
        }
    }

    /// Applies the requests held while a step was out.
    fn apply_held(&mut self) {
        for text in core::mem::take(&mut self.held) {
            let reply = self.handle(&text);
            println!("goals: a held request, answered later: {reply}");
        }
    }

    /// One `goal-*` request from the shell.
    fn handle(&mut self, text: &str) -> String {
        let (command, rest) = text.split_once([' ', '\n']).unwrap_or((text, ""));
        let id = || {
            rest.trim()
                .parse::<u64>()
                .map_err(|_| String::from("a goal id"))
        };
        let now = now_ms();
        let done = match command {
            "goal-add" => self.add(rest),
            "goal-pause" => id().and_then(|id| self.with(id, |goal| goal.pause())),
            "goal-resume" => id().and_then(|id| self.goals.resume(id).map_err(|why| why.words())),
            "goal-cancel" => id().and_then(|id| {
                self.planner.forget(id);
                self.with(id, |goal| goal.cancel(now))
            }),
            "goal-list" => return format!("ok {}", self.list()),
            _ => Err(format!("unknown request {command:?}")),
        };
        match done {
            Ok(()) => String::from("ok"),
            Err(why) => format!("err {why}"),
        }
    }

    /// `goal-add`: a `kai-goals 1` file of the one goal the person approved, kept
    /// under the shell's id.
    fn add(&mut self, file: &str) -> Result<(), String> {
        let file = Goals::parse(file).map_err(|why| format!("not a kai-goals 1 file: {why}"))?;
        match file.list() {
            [goal] => self.goals.adopt(goal.clone()).map_err(|why| why.words()),
            _ => Err(String::from("goal-add takes one goal")),
        }
    }

    fn with(
        &mut self,
        id: u64,
        change: impl FnOnce(&mut Goal) -> Result<(), goalcore::Refusal>,
    ) -> Result<(), String> {
        let goal = self
            .goals
            .get_mut(id)
            .ok_or_else(|| format!("no goal {id}"))?;
        change(goal).map_err(|why| why.words())
    }

    fn list(&self) -> String {
        let lines: Vec<String> = self
            .goals
            .list()
            .iter()
            .map(|goal| {
                format!(
                    "{} {} {}/{} steps: {}",
                    goal.id,
                    goal.state.word(),
                    goal.steps_run,
                    goal.budget.steps,
                    goal.text
                )
            })
            .collect();
        match lines.is_empty() {
            true => String::from("no goals"),
            false => lines.join("\n"),
        }
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

/// The shell's door for this agent's steps. While a step is out, the shell's own
/// `goal-*` requests are taken and held for after the turn.
struct ShellSteps<'a> {
    held: &'a mut Vec<String>,
}

impl Steps<String> for ShellSteps<'_> {
    fn run(&mut self, goal: u64, step: &Step<String>) -> StepDone {
        let sent = format!("agent-step goal={goal} {}", step.call);
        let reply = match self.call_shell(&sent) {
            Ok(reply) => reply,
            Err(why) => return failed(None, why),
        };
        let reply = match reply.strip_prefix("ok asked ") {
            Some(rest) => {
                let id = rest.split(':').next().unwrap_or("").trim().to_string();
                match self.wait_for(&id) {
                    Ok(answer) => answer,
                    Err(why) => return failed(None, why),
                }
            }
            None => reply,
        };
        step_done(&reply)
    }
}

impl ShellSteps<'_> {
    /// Polls `agent-step wait <id>` until the person answered on the card.
    fn wait_for(&mut self, id: &str) -> Result<String, String> {
        loop {
            let reply = self.call_shell(&format!("agent-step wait {id}"))?;
            if !reply.starts_with("ok waiting") {
                return Ok(reply);
            }
            self.hold_requests(WAIT_POLL_MS);
        }
    }

    /// One call to the shell, sent without blocking: while it is out, the shell's
    /// own requests are taken and held, so the two never wait on each other.
    fn call_shell(&mut self, text: &str) -> Result<String, String> {
        let shell = route(SHELL).ok_or("no route to the shell (uses=llm+shell)")?;
        sys::call_submit(shell, text.as_bytes(), STEP_MS)
            .map_err(|error| format!("shell: {error:?}"))?;
        let mut reply = alloc::vec![0u8; kabi::MAX_MESSAGE];
        loop {
            match sys::call_poll(&mut reply, 0) {
                Ok(len) => {
                    let len = len.min(reply.len());
                    return Ok(String::from_utf8_lossy(&reply[..len]).into_owned());
                }
                Err(kabi::Error::Busy) => self.hold_requests(POLL_MS),
                Err(error) => return Err(format!("shell: {error:?}")),
            }
        }
    }

    /// Takes the shell's requests for up to `wait_ms`: each is answered `ok held`
    /// and applied after the turn.
    fn hold_requests(&mut self, wait_ms: u64) {
        let mut request = alloc::vec![0u8; kabi::MAX_MESSAGE];
        let mut info = RecvInfo::default();
        let Ok(len) = recv(&mut request, wait_ms, &mut info) else {
            return;
        };
        let text = String::from_utf8_lossy(&request[..len.min(request.len())]).into_owned();
        let reply: &[u8] = match info.badge == KAI {
            true => {
                self.held.push(text);
                b"ok held: a step is out; it applies after it"
            }
            false => b"err only the shell hands goals over",
        };
        if let Err(error) = sys::svc_reply(reply) {
            println!("goals: svc_reply failed: {error:?}");
        }
    }
}

/// What the shell's reply to a step says: the entry and whether it ran.
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

/// The next request, with who sent it; handles a request moved are closed.
fn recv(request: &mut [u8], timeout_ms: u64, info: &mut RecvInfo) -> Result<usize, kabi::Error> {
    let len = sys::recv_h(request, timeout_ms, info)?;
    let count = (info.handle_count as usize).min(info.handles.len());
    for &handle in &info.handles[..count] {
        let _ = sys::handle_close(Handle(handle));
    }
    Ok(len)
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
