//! The `goals` agent, as a stub until row G1 of `docs/plans/agents-os.md` builds
//! it: ADR 0013's built-in agent `goals`, whose route to `llm` carries badge 2
//! (`agent=goals` on its plan line). With no arguments it only waits.
//!
//! `test=agent-job` is the `agent-job` boot case's client (F3 slices 2a and 2b):
//! through its own route it starts an `ask` and an `act` job, takes each answer, and
//! checks that a ticket it never had reads `ended unknown ticket`. It prints one
//! line a step, which the case reads; the person's line after the `act` is the
//! case's own, typed at the shell.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use llmproto::act::ActRequest;
use user::{arg, args, println, route, sys};

user::entry!(main);

/// How long one call to `llm` may take: an answer is taken later, so a call only
/// queues or reads.
const CALL_MS: u64 = 10_000;
/// How long an answer is waited for, and how often it is asked for.
const ANSWER_MS: u64 = 300_000;
const POLL_MS: u64 = 200;

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
        _ => {
            println!("up; goals stub (row G1 builds the agent)");
            loop {
                sys::sleep_ms(60_000);
            }
        }
    }
}

/// One request to `llm` on this agent's route, and its reply.
fn call(text: &str) -> Result<String, String> {
    let llm = route(0).ok_or("no route to llm (uses=llm)")?;
    let mut reply = alloc::vec![0u8; kabi::MAX_MESSAGE];
    let len = sys::call_route(llm, text.as_bytes(), &mut reply, CALL_MS)
        .map_err(|error| format!("llm: {error:?}"))?;
    Ok(String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned())
}

/// Starts a job of `kind` and waits for its answer.
fn job(kind: &str, request: &str) -> Result<String, String> {
    let started = call(&format!("agent-start {kind} {request}"))?;
    let ticket: String = started
        .strip_prefix("ok ")
        .ok_or_else(|| format!("agent-start {kind}: {started}"))?
        .trim()
        .into();
    println!("goals: {kind} job {ticket} queued");
    let began = sys::time_ns();
    loop {
        let taken = call(&format!("agent-take {ticket}"))?;
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
        line: String::from("how many cpus are there?"),
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
    let unknown = call("agent-take 999999")?;
    if unknown != "ended unknown ticket" {
        return Err(format!("a stranger's ticket read {unknown:?}"));
    }
    println!("goals: unknown ticket refused");
    Ok(())
}
