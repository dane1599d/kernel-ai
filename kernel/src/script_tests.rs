//! M8b self-test (`test=scripts`): LLM-written scripts end to end. The shell asks the
//! llm service, which asks the mock LLM server (its answers are in xtask's
//! `mock_llm`). The kernel checks every reply, that the policy changes only after a
//! yes, and how many requests reached the llm service, which counts repair rounds.

use crate::process::syscall::MIN_POLICY_INTERVAL_NS;
use crate::shell_tests::Expect::{self, Contains, Exact, StartsWith};
use crate::shell_tests::{expect_lines, start};
use crate::{ipc, log, threads};

/// A script is shown in full and runs once; a no runs nothing.
const DECLINE: &[(&str, Expect)] = &[
    (
        "? how many cpus are there",
        Exact(
            "confirm 1 script 1: reads cpus\nmodel says: Prints the number of CPUs.\n 1 | print(\"CPUs: \" + cpus());\nrun it? it changes nothing. yes/no",
        ),
    ),
    ("answer 1 yes", StartsWith("ok CPUs: ")),
    ("answer 1 yes", Exact("err nothing to confirm")),
    (
        "? switch to round robin",
        StartsWith("confirm 2 script 2: reads get_policy; CHANGES set_policy\n"),
    ),
    ("answer 2 no", Exact("ok script 2 not run")),
];

/// A yes runs the change, and the script sees it at once: it reads the policy just
/// before switching, so a stale metrics snapshot would print the old one.
const ACCEPT: &[(&str, Expect)] = &[
    ("? switch to round robin", StartsWith("confirm 3 ")),
    ("answer 3 yes", Exact("ok policy: round-robin\n")),
];

const REST: &[(&str, Expect)] = &[
    // The first answer calls a function that does not exist; the repair fixes it.
    (
        "? how much memory is free",
        StartsWith("confirm 4 script 4: reads memory\n"),
    ),
    ("answer 4 yes", Contains(" MiB free")),
    (
        "? reboot the computer",
        Exact("err the model wrote no script: There is no way to reboot from a script."),
    ),
    // Plain English that is not Rhai goes to the LLM too. The mock answers it with
    // prose instead of the JSON envelope, so the shell gives up after one repair.
    (
        "tell me a story please",
        Exact("err gave up after one repair: the answer was not the JSON object asked for"),
    ),
    ("? go back to interactive", StartsWith("confirm 7 ")),
    ("answer 7 yes", Exact("ok ")),
    (
        "log",
        StartsWith(
            "ok #1 how many cpus are there: ran\n#2 switch to round robin: declined\n#3 switch to round robin: ran\n#4 how much memory is free: ran\n#5 reboot the computer: refused: ",
        ),
    ),
];

/// Script requests the lines above make: one each, and two for the repaired ones
/// (4 and 6).
const LLM_REQUESTS: u64 = 9;

fn policy_is(name: &str) -> bool {
    for _ in 0..500 {
        if threads::installed_policy_everywhere(name) {
            return true;
        }
        threads::sleep_ms(10);
    }
    false
}

pub fn check() -> Result<(), &'static str> {
    let shell = start()?;
    let before = ipc::replies("llm").1;
    expect_lines(DECLINE)?;
    if !policy_is("interactive") {
        return Err("scripts: a declined script changed the policy");
    }
    expect_lines(ACCEPT)?;
    if !policy_is("round-robin") {
        return Err("scripts: an accepted script did not change the policy");
    }
    // Policy switches are rate-limited; the last script switches back, so wait the
    // limit out rather than rely on how long the lines in between take.
    threads::sleep_ms(MIN_POLICY_INTERVAL_NS / 1_000_000 + 10);
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
        "scripts: proposed, declined, accepted and repaired scripts all behaved (shell pid {shell})"
    );
    Ok(())
}
