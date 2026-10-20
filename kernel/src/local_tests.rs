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
//!
//! `test=local-script`, L5's exit test, is [`check_script`].

use crate::{hpet, ipc, llm, log, model, process, terminal, threads};
use kabi::ExitReason;
use kabi::model::fnv1a;

/// How long the services get to come up, the typed question to be answered, and
/// the supervisor to restart llm.
const WAIT_MS: u64 = 60_000;
/// The question the harness types, and the kernel asks again.
const QUESTION: &str = "hi";
/// What the harness types for `local-script`, and the kernel sends the shell again.
const SCRIPT_REQUEST: &str = "show me the cpus please";
/// How long one script request may take: two rounds of the shell's 70 s, with room.
const SCRIPT_WAIT_MS: u64 = 150_000;

/// The hash xtask put on the command line: of the golden answer for `local-ask`,
/// of the shell's golden reply for `local-script`.
fn golden(cmdline: &str) -> Option<u64> {
    cmdline
        .split_whitespace()
        .find_map(|word| word.strip_prefix("local_golden="))
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
}

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    if llm::target().is_some() {
        return Err("local-ask: the boot names an LLM server; this case runs without one");
    }
    let golden = golden(cmdline).ok_or("local-ask: the command line has no local_golden=")?;
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
    // L6: the engine runs on one pool worker per CPU online.
    if !health.contains(&alloc::format!("{} workers", crate::smp::online())) {
        return Err("local-ask: the engine does not run one worker per CPU");
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

/// `test=local-script`: a plain-English request typed at the terminal reaches the
/// shell, which asks llm for a script; llm writes it with the model on the
/// `kai-model` disk, decoded as the JSON envelope, with no LLM server; the shell
/// checks it and refuses or proposes it by its own rules, unchanged. xtask
/// worked out that outcome on the host from the same file, checks the terminal
/// showed exactly it, and that its mock server got no request. The kernel then
/// sends the shell the same request itself, which the same llm process answers
/// with the engine that kept its prompt cache, and checks the reply hashes to the
/// one xtask put on the command line (`local_golden=`), in as many rounds, and
/// that llm's last answer took prompt tokens from the cache.
pub fn check_script(cmdline: &str) -> Result<(), &'static str> {
    if llm::target().is_some() {
        return Err("local-script: the boot names an LLM server; this case runs without one");
    }
    let golden = golden(cmdline).ok_or("local-script: the command line has no local_golden=")?;
    if model::get().is_none() {
        return Err("local-script: the kai-model disk was not loaded");
    }
    wait_for(|| ipc::server_of("llm").is_some() && ipc::server_of("shell").is_some())
        .map_err(|_| "local-script: llm and the shell did not come up")?;
    let health = llm::health().map_err(|_| "local-script: llm's health failed")?;
    if !health.contains("local model ") {
        return Err("local-script: health does not name a local model");
    }
    let (pid, maps) = (ipc::server_of("llm"), model::maps());
    threads::spawn("terminal", terminal::run, 0)
        .map_err(|_| "local-script: cannot start the terminal")?;
    threads::sleep_ms(200);
    let shown = terminal::replies_shown();
    log!("local-script: ready for keys");
    wait_within(SCRIPT_WAIT_MS, || terminal::replies_shown() > shown)
        .map_err(|_| "local-script: the typed request was never answered")?;
    let rounds = ipc::replies("llm").1;
    log!("local-script: the typed request was answered in {rounds} script rounds");
    let reply = ipc::call("shell", SCRIPT_REQUEST.as_bytes(), SCRIPT_WAIT_MS)
        .map_err(|_| "local-script: the shell did not answer the kernel")?;
    let text = core::str::from_utf8(&reply).unwrap_or("");
    if fnv1a(text.as_bytes()) != golden {
        log!("local-script: the shell answered the kernel: {text}");
        return Err("local-script: the second request did not get the host's outcome");
    }
    if rounds == 0 || ipc::replies("llm").1 != 2 * rounds {
        return Err("local-script: the second request took another number of rounds");
    }
    if (ipc::server_of("llm"), model::maps()) != (pid, maps) {
        return Err("local-script: llm restarted between the requests, losing its cache");
    }
    let health = llm::health().map_err(|_| "local-script: llm's health failed")?;
    log!("local-script: health: {health}");
    if cached_prompt_tokens(&health).is_none_or(|cached| cached == 0) {
        return Err("local-script: the second request computed its whole prompt again");
    }
    log!("local-script: the local model's script got the host's outcome, twice, with no server");
    Ok(())
}

/// The prompt tokens llm's last answer took from its cache, from `health`'s
/// `N of M prompt tokens from the cache`.
fn cached_prompt_tokens(health: &str) -> Option<usize> {
    let before = health.split(" prompt tokens from the cache").next()?;
    let (count, _) = before.rsplit_once(" of ")?;
    count.rsplit(' ').next()?.parse().ok()
}

/// Polls `done` every millisecond, so a call llm takes is seen while it runs, for
/// up to [`WAIT_MS`].
fn wait_for(done: impl Fn() -> bool) -> Result<(), ()> {
    wait_within(WAIT_MS, done)
}

/// Polls `done` every millisecond for up to `ms`.
fn wait_within(ms: u64, done: impl Fn() -> bool) -> Result<(), ()> {
    let deadline = hpet::nanos() + ms * 1_000_000;
    while !done() {
        if hpet::nanos() >= deadline {
            return Err(());
        }
        threads::sleep_ms(1);
    }
    Ok(())
}
