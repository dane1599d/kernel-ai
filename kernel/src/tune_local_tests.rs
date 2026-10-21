//! `tune` with the model inside Kernel AI (M10a-4).
//!
//! `test=tune-local workload=A`: the generated tune model on the `kai-model`
//! disk and no server. The llm service's answer to `llmproto::tune::example()`,
//! decoded under the answer's schema, must hash to the host engine's answer that
//! xtask put on the command line (`local_golden=`); then the shell's `tune
//! editor` runs end to end on that model, whatever it decides (a proposal is
//! declined).
//!
//! `test=tune-demo workload=A`: the founder's demo, for `tune-model.yml` with the
//! real model on the disk. The harness types `tune editor` at the terminal, `yes`
//! once the proposal card waits for it, and asks QEMU for a screendump after the
//! verdict. The llm service traces every request and answer, and `cargo xtask
//! tune check` requires the guest's answer to be the host engine's.

use crate::sched_control;
use crate::tune_tests::{finished, line, pid_named};
use crate::{ipc, llm, local_tests, log, model, terminal, threads};
use kabi::model::fnv1a;
use kabi::sched::state;

/// How long the llm service may take for one tune answer.
const ANSWER_MS: u64 = 150_000;

pub fn local_check(cmdline: &str) -> Result<(), &'static str> {
    if llm::target().is_some() {
        return Err("local-tune: the boot names an LLM server; this case runs without one");
    }
    let golden =
        local_tests::golden(cmdline).ok_or("local-tune: the command line has no local_golden=")?;
    if model::get().is_none() {
        return Err("local-tune: the kai-model disk was not loaded");
    }
    crate::shell_tests::start()?;
    // xtask checks this line names the generated model, as for local-ask.
    let health = llm::health().map_err(|_| "local-tune: llm's health failed")?;
    log!("local-tune: health: {health}");
    if !health.contains("local model ") {
        return Err("local-tune: health does not name a local model");
    }
    let request = llmproto::tune::example().encode();
    let reply = ipc::call("llm", request.as_bytes(), ANSWER_MS)
        .map_err(|_| "local-tune: llm did not answer the tune request")?;
    let reply = alloc::string::String::from_utf8_lossy(&reply).into_owned();
    let answer = reply
        .strip_prefix("ok ")
        .ok_or("local-tune: llm refused the request")?;
    if fnv1a(answer.as_bytes()) != golden {
        log!("local-tune: llm answered {answer:?}");
        return Err("local-tune: the local model's answer is not the host engine's");
    }
    log!("local-tune: the local model's tune answer equals the host engine's, byte for byte");
    pid_named("editor")?;
    threads::sleep_ms(1_000);
    let reply = line("tune editor")?;
    if let Some(rest) = reply.strip_prefix("confirm ") {
        let id = rest.split(' ').next().unwrap_or("");
        if line(&alloc::format!("answer {id} no"))? != alloc::format!("ok proposal {id} not tried")
        {
            return Err("local-tune: declining the proposal did not answer as expected");
        }
    } else if !reply.starts_with("ok ") {
        return Err("local-tune: the shell's tune failed on the local model");
    }
    log!("local-tune: the shell's tune ran end to end on the local model");
    Ok(())
}

/// Waits up to `ms` for `done`.
fn wait(ms: u64, done: impl Fn() -> bool) -> bool {
    (0..ms / 50).any(|_| {
        threads::sleep_ms(50);
        done()
    })
}

pub fn demo_check() -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    crate::shell_tests::start()?;
    pid_named("editor")?;
    threads::sleep_ms(2_000);
    let shown = terminal::replies_shown();
    log!("tune-demo: ready for keys");
    // The observe window, the model's answer, then the card or a keep.
    if !wait(600_000, || {
        terminal::awaiting_answer() || terminal::replies_shown() > shown
    }) {
        return Err("tune-demo: the terminal never showed the model's answer");
    }
    let trial = sched_control::status().id;
    if terminal::awaiting_answer() {
        log!("tune-demo: ready for keys");
        if !wait(30_000, || sched_control::status().id > trial) {
            return Err("tune-demo: the typed yes started no trial");
        }
        let status = finished(sched_control::status().id)?;
        let shown = terminal::replies_shown();
        wait(10_000, || terminal::replies_shown() > shown);
        log!(
            "tune-demo: trial {} ended: {}",
            status.id,
            if status.state == state::KEPT {
                "kept"
            } else {
                "rolled back"
            }
        );
    } else {
        log!("tune-demo: the model kept the current policy");
        // The harness's `yes` still comes; the shell just reports it as no command.
        log!("tune-demo: ready for keys");
        threads::sleep_ms(2_000);
    }
    log!("tune-demo: ready for keys");
    threads::sleep_ms(3_000);
    log!("tune-demo: done");
    Ok(())
}
