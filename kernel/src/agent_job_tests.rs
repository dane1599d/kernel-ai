//! `test=agent-job` (ADR 0013 section 2; F3 slices 2a and 2b of
//! `docs/plans/agents-os.md`): an agent's requests are jobs on its own route to
//! `llm`, whose badge the kernel sets. The `goals` stub, planned with
//! `agent=goals`, starts an `ask` and an `act` job, takes their answers, and checks
//! that a ticket it never had is unknown; it prints `goals: agent-job ok` and
//! exits, which the case reads on serial. Then the person types a line, the first
//! after the agent's `act`, which pays the act prefix's re-warm (on a local model;
//! the mock server has no prefix), and its time is logged.

use crate::shell_tests::start;
use crate::{ipc, log, process, threads};
use alloc::format;
use alloc::string::String;

/// The `goals` agent's plan line (lane K's), run once for the case.
pub const GOALS: &str = "goals caps=call uses=llm agent=goals pages=1024 stack_pages=64 burst_ms=10000 unsupervised test=agent-job";
/// The person's line after the agent's jobs, one the mock server answers.
const LINE: &str = "how many processors do I have";
/// How long the agent's jobs may take, under TCG too.
const AGENT_WAIT_MS: u64 = 300_000;
const LINE_MS: u64 = 120_000;
const POLL_MS: u64 = 100;

/// llm first, as the route's target must be; the shell comes after, as for every
/// case with a shell.
pub fn plan() -> String {
    format!("{}\n{GOALS}", crate::llm::plan_line())
}

pub fn check() -> Result<(), &'static str> {
    start()?;
    // The agent runs, and exits once its jobs are answered.
    let mut seen = false;
    for _ in 0..AGENT_WAIT_MS / POLL_MS {
        let running = !process::named("goals").is_empty();
        seen |= running;
        if seen && !running {
            break;
        }
        threads::sleep_ms(POLL_MS);
    }
    if !seen {
        return Err("agent-job: the goals agent never ran");
    }
    if !process::named("goals").is_empty() {
        return Err("agent-job: the goals agent's jobs were not answered in time");
    }
    log!("agent-job: the goals agent's jobs are done");
    // The person's line, right after the agent's act.
    let began = crate::hpet::nanos();
    let reply = ipc::call("shell", LINE.as_bytes(), LINE_MS)
        .map_err(|_| "agent-job: the person's line got no reply")?;
    let ms = (crate::hpet::nanos() - began) / 1_000_000;
    let reply = String::from_utf8_lossy(&reply);
    if !reply.starts_with("ok ") {
        log!("agent-job: the person's line answered {reply:?}");
        return Err("agent-job: the person's line was not answered");
    }
    log!("agent-job: the person's line after an agent's act took {ms} ms");
    Ok(())
}
