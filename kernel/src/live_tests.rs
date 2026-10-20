//! M9a self-test (`test=live`): the shared scenario (`livecore::scenario`) run by
//! the kernel against the real `notes` host process. Every check reads a reply,
//! the kernel's own ledger and store, or the IPC layer's counts.

use crate::process::{self, components};
use crate::{hpet, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use kabi::ExitReason;
use livecore::ledger::Ledger;
use livecore::scenario::{self, Driver};

const NAME: &str = "notes";
const START_TIMEOUT_MS: u64 = 60_000;
const CALL_TIMEOUT_MS: u64 = 60_000;
/// The caller a spoofed control request claims to be.
pub(crate) const SPOOF_PID: u64 = 4242;

pub(crate) struct Kernel {
    pub(crate) calls: u64,
}

fn text(reply: &[u8]) -> String {
    String::from_utf8_lossy(reply).into_owned()
}

/// Waits for `notes` to be served by a process other than `old`.
pub(crate) fn wait_for_host(old: u64) -> Result<u64, String> {
    for _ in 0..START_TIMEOUT_MS / 10 {
        if let Some(pid) = ipc::server_of(NAME)
            && pid != old
        {
            return Ok(pid);
        }
        threads::sleep_ms(10);
    }
    Err(format!("no new {NAME} host within {START_TIMEOUT_MS} ms"))
}

impl Driver for Kernel {
    fn call(&mut self, request: &str) -> Result<String, String> {
        self.calls += 1;
        ipc::call(NAME, request.as_bytes(), CALL_TIMEOUT_MS)
            .map(|reply| text(&reply))
            .map_err(|error| format!("{request:?} got no reply: {error:?}"))
    }

    fn spoof(&mut self, request: &str) -> Result<String, String> {
        let deadline = hpet::nanos() + CALL_TIMEOUT_MS * 1_000_000;
        ipc::call_as(NAME, request.as_bytes(), SPOOF_PID, deadline, || false)
            .map(|reply| text(&reply))
            .map_err(|error| format!("{request:?} got no reply: {error:?}"))
    }

    fn ledger(&mut self) -> Ledger {
        components::ledger(NAME).unwrap_or_default()
    }

    fn fetches(&mut self, label: &str) -> u64 {
        components::fetches(NAME, label)
    }

    fn host_pid(&mut self) -> u64 {
        ipc::server_of(NAME).unwrap_or(0)
    }

    fn restart(&mut self) -> Result<(), String> {
        let old = self.host_pid();
        process::kill(old, ExitReason::Killed)
            .map_err(|error| format!("cannot kill pid {old}: {error:?}"))?;
        wait_for_host(old).map(|_| ())
    }

    fn log(&mut self, text: &str) {
        log!("{text}");
    }
}

pub fn check() -> Result<(), &'static str> {
    if let Err(why) = wait_for_host(0) {
        log!("{why}");
        return Err("live: the notes host never came up");
    }
    let before = ipc::replies(NAME);
    let mut kernel = Kernel { calls: 0 };
    if let Err(why) = scenario::run(&mut kernel) {
        log!("{why}");
        return Err("live: the scenario failed");
    }
    // Every call the kernel made was answered, and only the spoof came from a process.
    let (to_kernel, to_processes) = ipc::replies(NAME);
    if (to_kernel - before.0, to_processes - before.1) != (kernel.calls, 1) {
        log!(
            "live: notes answered {} kernel and {} process calls; the kernel made {}",
            to_kernel - before.0,
            to_processes - before.1,
            kernel.calls
        );
        return Err("live: the reply counts are wrong");
    }
    log!(
        "live: {} steps passed: swapped, migrated, rolled back and restarted ({} calls)",
        scenario::STEPS.len(),
        kernel.calls
    );
    // `:state notes`: the restored state reads as notes 2's schema.
    let shown = crate::live_commands::state(NAME, &kernel.ledger());
    for line in &shown {
        log!("{line}");
    }
    if !livecore::state::shows_schema(&shown, 2) {
        return Err("live: :state notes does not show schema 2");
    }
    // `:why notes`: the mock LLM repeats the rollback the ledger's facts named, the
    // last one of the scenario, so the facts reached the model. With no server
    // (the `live-local` case) the model on the `kai-model` disk answers instead,
    // and xtask checks its mock got no request.
    if let Err(why) = wait_for("llm") {
        log!("{why}");
        return Err("live: the llm service never came up");
    }
    let shown = crate::live_commands::why(NAME, &kernel.ledger());
    for line in &shown {
        log!("{line}");
    }
    match crate::llm::target() {
        Some(_) => {
            let explained = shown
                .iter()
                .any(|line| line.starts_with("= ") && line.contains(LATEST_ROLLBACK));
            if !explained {
                return Err("live: :why notes does not explain the latest rollback");
            }
        }
        None => why_answered_locally(&shown)?,
    }
    // `:fault notes` with no build service: the lines around 3-trap's panic, from
    // the kernel's own store.
    let shown = crate::edit_mode::fault(NAME);
    crate::edit_mode::show(&shown);
    let marked = shown.lines().any(|line| line.starts_with('>'));
    if !shown.starts_with(FAULT_AT) || !marked {
        return Err("live: :fault notes does not show 3-trap's line");
    }
    Ok(())
}

/// `:why`'s answer came from the local model: llm runs one, and showed an
/// explanation, whatever words the generated model chose.
fn why_answered_locally(shown: &[String]) -> Result<(), &'static str> {
    let health = crate::llm::health().map_err(|_| "live: llm's health failed")?;
    log!("live: health: {health}");
    if !health.contains("local model ") {
        return Err("live: no server and no local model, yet :why was asked");
    }
    if !shown.iter().any(|line| line.starts_with("= ")) {
        return Err("live: the local model did not explain the rollback");
    }
    log!("live: :why notes was answered by the local model");
    Ok(())
}

/// What `:fault notes` names after the scenario: 3-trap's panic, in part C.
const FAULT_AT: &str = "ok notes 3-trap trapped at components/fixtures/notes-bad/src/lib.rs:";

/// What the facts say about the scenario's last rollback, in part F.
const LATEST_ROLLBACK: &str = "latest rollback: 3-spin (host-restart)";

/// Waits for service `name` to be up.
fn wait_for(name: &str) -> Result<(), String> {
    for _ in 0..START_TIMEOUT_MS / 10 {
        if ipc::server_of(name).is_some() {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    Err(format!("no {name} service within {START_TIMEOUT_MS} ms"))
}
