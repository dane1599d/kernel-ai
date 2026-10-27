//! M8b self-test (`test=scripts`): LLM-written scripts end to end. The shell asks the
//! llm service, which asks the mock LLM server (its answers are in xtask's
//! `mock_llm`). The kernel checks every reply: run-tier scripts run at once and say
//! what they did, `show` prints the last script, `undo` puts the policy back, and
//! the number of requests that reached the llm service counts repair rounds.

use crate::sched_control::calls::MIN_POLICY_INTERVAL_NS;
use crate::shell_tests::Expect::{self, Contains, Exact, StartsWith};
use crate::shell_tests::{expect_lines, start};
use crate::{ipc, log, threads};

/// A script that only reads runs at once and shows only its output; `show` has the
/// script. A policy switch is run-tier too, and the script sees it at once: it
/// reads the policy just before switching, so a stale metrics snapshot would print
/// the old one.
const RUN: &[(&str, Expect)] = &[
    ("? how many cpus are there", StartsWith("ok CPUs: ")),
    ("answer 1 yes", Exact("err nothing to confirm")),
    (
        "show",
        Exact(
            "ok script 1: reads cpus\nmodel says: Prints the number of CPUs.\n 1 | print(\"CPUs: \" + cpus());\nran at once: it only reads",
        ),
    ),
    (
        "? switch to round robin",
        Exact("ok policy: round-robin\nDone: policy round-robin (was interactive). undo"),
    ),
];

/// `undo` puts the policy back; a bare `y` with nothing asked answers nothing.
/// (A second `undo` would redo: the undo is an entry of its own, `act-undo`.)
const UNDO: &[(&str, Expect)] = &[
    ("undo", Exact("ok Undone: policy back to interactive")),
    ("y", Exact("err nothing to answer")),
];

const AGAIN: &[(&str, Expect)] = &[(
    "? switch to round robin",
    Contains("Done: policy round-robin"),
)];

const REST: &[(&str, Expect)] = &[
    // The first answer calls a function that does not exist; the repair fixes it.
    // The output, which the script's own text (`" MiB free");`) cannot match.
    ("? how much memory is free", Contains(" MiB free\n")),
    // No script: the model answers in words.
    (
        "? reboot the computer",
        Exact("ok There is no way to reboot from a script."),
    ),
    // Plain English that is not Rhai goes to the LLM too. The mock answers it with
    // prose instead of the JSON envelope, so the shell gives up after one repair.
    (
        "tell me a story please",
        Exact("err gave up after one repair: the answer was not the JSON object asked for"),
    ),
    (
        "? go back to interactive",
        Exact("ok Done: policy interactive (was round-robin). undo"),
    ),
    (
        "log",
        Exact(
            "ok #1 how many cpus are there: ran read-only\n#2 switch to round robin: ran\n#3 undo of #2: done\n#4 switch to round robin: ran\n#5 how much memory is free: ran read-only\n#6 reboot the computer: answered in words\n#7 tell me a story please: refused: gave up after one repair: the answer was not the JSON object asked for\n#8 go back to interactive: ran",
        ),
    ),
];

/// Requests the lines above make of the llm service: for each of the seven, an
/// `act` request the mock answers with no call, then the script request, which is
/// two for the repaired ones (4 and 6).
const LLM_REQUESTS: u64 = 16;

fn policy_is(name: &str) -> bool {
    for _ in 0..500 {
        if threads::installed_policy_everywhere(name) {
            return true;
        }
        threads::sleep_ms(10);
    }
    false
}

/// Policy switches are rate-limited, so wait the limit out between lines that
/// switch rather than rely on how long the lines in between take.
fn after_switch() {
    threads::sleep_ms(MIN_POLICY_INTERVAL_NS / 1_000_000 + 10);
}

pub fn check() -> Result<(), &'static str> {
    let shell = start()?;
    let before = ipc::replies("llm").1;
    expect_lines(RUN)?;
    if !policy_is("round-robin") {
        return Err("scripts: a run-tier script did not switch the policy at once");
    }
    // P1 task 14: the scripts ran in the shell's pooled `scriptrun` child, which
    // waits there for the next one; the pool names its children `scriptrun-1`
    // to `scriptrun-4` (`process::job`).
    let pooled = (1..=4).any(|k| {
        crate::process::named(&alloc::format!("scriptrun-{k}"))
            .iter()
            .any(|&(_, supervised)| !supervised)
    });
    if !pooled {
        return Err("scripts: no scriptrun child of the shell ran them");
    }
    after_switch();
    expect_lines(UNDO)?;
    if !policy_is("interactive") {
        return Err("scripts: undo did not put the policy back");
    }
    after_switch();
    expect_lines(AGAIN)?;
    if !policy_is("round-robin") {
        return Err("scripts: the second switch did not change the policy");
    }
    after_switch();
    expect_lines(REST)?;
    if !policy_is("interactive") {
        return Err("scripts: the last script did not restore the policy");
    }
    let requests = ipc::replies("llm").1 - before;
    if requests != LLM_REQUESTS {
        log!("scripts: the llm service answered {requests} script requests, not {LLM_REQUESTS}");
        return Err("scripts: wrong number of LLM requests");
    }
    if ipc::server_of("shell") != Some(shell) {
        return Err("scripts: a different shell process answered by the end");
    }
    log!(
        "scripts: run-tier scripts ran at once, show, undo and repairs all behaved (shell pid {shell})"
    );
    Ok(())
}
