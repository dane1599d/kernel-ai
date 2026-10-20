//! `test=local-ask`, L4's exit test: `ask` is answered by the model on the
//! `kai-model` disk, run inside Kernel AI by the ring-3 llm service, with no LLM
//! server on the command line.
//!
//! The harness types `ask hi` at the terminal, which goes through the shell to the
//! llm service; xtask checks the terminal showed exactly the answer it computed on
//! the host from the same file, and that its mock server got no request. The kernel
//! checks the rest: `health` names the local model; the kernel's own `ask hi` gets
//! the answer whose FNV-1a hash xtask put on the command line (`local_golden=`);
//! and when llm is killed in the middle of an answer, the caller gets an error at
//! once, the supervisor restarts llm, and the new process maps the model the boot
//! copied, never a second copy, and answers the same.

use crate::{hpet, ipc, llm, log, model, process, terminal, threads};
use kabi::ExitReason;
use kabi::model::fnv1a;

/// How long the services get to come up, the typed question to be answered, and
/// the supervisor to restart llm.
const WAIT_MS: u64 = 60_000;
/// The question the harness types, and the kernel asks again.
const QUESTION: &str = "hi";

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    if llm::target().is_some() {
        return Err("local-ask: the boot names an LLM server; this case runs without one");
    }
    let golden = cmdline
        .split_whitespace()
        .find_map(|word| word.strip_prefix("local_golden="))
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .ok_or("local-ask: the command line has no local_golden=")?;
    if model::get().is_none() {
        return Err("local-ask: the kai-model disk was not loaded");
    }
    wait_for(|| ipc::server_of("llm").is_some() && ipc::server_of("shell").is_some())
        .map_err(|_| "local-ask: llm and the shell did not come up")?;
    let health = llm::health().map_err(|error| {
        log!("local-ask: health failed: {error}");
        "local-ask: llm's health failed"
    })?;
    log!("local-ask: health: {health}");
    if !health.contains("local model ") {
        return Err("local-ask: health does not name a local model");
    }
    typed_question()?;
    let answer = asked("the kernel's ask")?;
    if fnv1a(answer.as_bytes()) != golden {
        log!("local-ask: the kernel's ask got: {answer}");
        return Err("local-ask: the kernel's ask did not get the golden answer");
    }
    log!(
        "local-ask: the kernel's ask got the golden answer ({} characters)",
        answer.chars().count()
    );
    kill_mid_answer(golden)
}

/// Starts the terminal and waits until the harness's `ask hi` was answered and
/// shown.
fn typed_question() -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)
        .map_err(|_| "local-ask: cannot start the terminal")?;
    threads::sleep_ms(200);
    let shown = terminal::replies_shown();
    log!("local-ask: ready for keys");
    // llm answers the shell, and the terminal shows the shell's reply.
    wait_for(|| ipc::replies("llm").1 >= 1 && terminal::replies_shown() > shown)
        .map_err(|_| "local-ask: the typed question was never answered")?;
    log!("local-ask: the typed question was answered through the shell");
    Ok(())
}

/// The kernel's own `ask hi`.
fn asked(who: &'static str) -> Result<alloc::string::String, &'static str> {
    llm::ask(QUESTION).map_err(|error| {
        log!("local-ask: {who} failed: {error}");
        "local-ask: llm did not answer the kernel"
    })
}

/// Kills llm while it answers, and checks the restarted process maps the model
/// again and answers the same.
fn kill_mid_answer(golden: u64) -> Result<(), &'static str> {
    let (loads, maps) = (model::loads(), model::maps());
    let pid = ipc::server_of("llm").ok_or("local-ask: llm is not serving")?;
    let deadline = hpet::nanos() + WAIT_MS * 1_000_000;
    let request = alloc::format!("ask {QUESTION}");
    let pending = ipc::submit("llm", request.as_bytes(), 0, deadline)
        .map_err(|_| "local-ask: cannot send llm a question")?;
    wait_for(|| ipc::serving_deadline(pid).is_some() || pending.done())
        .map_err(|_| "local-ask: llm never took the question")?;
    if pending.done() {
        return Err("local-ask: llm answered before it could be killed");
    }
    process::kill(pid, ExitReason::Killed).map_err(|_| "local-ask: cannot kill llm")?;
    wait_for(|| pending.done()).map_err(|_| "local-ask: the killed call never ended")?;
    if ipc::finish(pending).is_ok() {
        return Err("local-ask: a call to the killed llm still got an answer");
    }
    wait_for(|| ipc::server_of("llm").is_some_and(|new| new != pid))
        .map_err(|_| "local-ask: the supervisor did not restart llm")?;
    let restarted = ipc::server_of("llm").unwrap_or(0);
    let answer = asked("the restarted llm's ask")?;
    if fnv1a(answer.as_bytes()) != golden {
        return Err("local-ask: the restarted llm did not give the golden answer");
    }
    let (loads_after, maps_after) = (model::loads(), model::maps());
    log!(
        "local-ask: killed llm (pid {pid}) mid-answer; restarted as pid {restarted}; \
         model loads {loads_after}, maps {maps} -> {maps_after}"
    );
    if loads_after != 1 || loads != 1 {
        return Err("local-ask: the model was copied from the disk more than once");
    }
    if maps_after != maps + 1 {
        return Err("local-ask: the restarted llm did not map the model exactly once");
    }
    log!("local-ask: answered locally; the restarted llm remapped the model, not reloaded it");
    Ok(())
}

/// Polls `done` every millisecond, so a call llm takes is seen while it runs, for
/// up to [`WAIT_MS`].
fn wait_for(done: impl Fn() -> bool) -> Result<(), ()> {
    let deadline = hpet::nanos() + WAIT_MS * 1_000_000;
    while !done() {
        if hpet::nanos() >= deadline {
            return Err(());
        }
        threads::sleep_ms(1);
    }
    Ok(())
}
