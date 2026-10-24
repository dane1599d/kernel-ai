//! P1 task 7's boot cases (`test=act-plain`, `act-run`, `act-never`,
//! `act-vague`): one tagged call through the one door. The shell sends `act` to
//! the llm service, which asks the mock LLM server; its calls are keyed by the
//! request line (`ACTS` in xtask's `mock_llm`). Code, not the model, decides what
//! each call may do, and writes the reply.

use crate::sched_control::calls::MIN_POLICY_INTERVAL_NS;
use crate::selftest::ActCase;
use crate::shell_tests::Expect::{self, Contains, Exact, StartsWith};
use crate::shell_tests::{expect_lines, start};
use crate::{log, threads};

/// Plain requests become the right calls: a change the line asks for, a read, and
/// `undo` of the change.
const PLAIN: &[(&str, Expect)] = &[
    (
        "turn on round robin scheduling",
        Exact("ok Done: policy round-robin (was interactive). undo"),
    ),
    ("how many processors do I have", StartsWith("ok ")),
];

const PLAIN_UNDO: &[(&str, Expect)] = &[("undo", Exact("ok Undone: policy back to interactive"))];

/// The run tier asks nothing: the reply is the result line itself, and `show`
/// has the one-line script code wrote for the call, with no model summary.
const RUN: &[(&str, Expect)] = &[
    (
        "turn on round robin scheduling",
        Exact("ok Done: policy round-robin (was interactive). undo"),
    ),
    ("show", Contains(" 1 | set_policy(\"round-robin\");")),
];

/// Never-tier calls are refused in code and change nothing.
const NEVER: &[(&str, Expect)] = &[
    (
        "please tune the shell process",
        Exact("err Refused tune: it is never run"),
    ),
    (
        "please tune the llm service",
        Exact("err Refused tune: it is never run"),
    ),
    ("log", Contains("refused: it is never run")),
];

/// After a switch to round-robin, a vague line runs its one undoable reading and
/// says what it took.
const VAGUE: &[(&str, Expect)] = &[(
    "turn on round robin scheduling",
    Exact("ok Done: policy round-robin (was interactive). undo"),
)];

const VAGUE_READING: &[(&str, Expect)] = &[(
    "make it snappier",
    Exact("ok Taken as: policy interactive (was round-robin). undo"),
)];

/// A change in a line that is not English asks first, and a no changes nothing.
const NOT_ENGLISH: &[(&str, Expect)] = &[
    (
        "cambia la política a round robin",
        Exact(
            "confirm 3 Run set_policy(\"round-robin\")? the request is not in English, so it asks first. y/n, or show",
        ),
    ),
    ("n", Contains("not run")),
];

/// S02-3 (ADR 0002): `undo` right after a switch, inside the kernel's 100 ms
/// limit, so the first try is refused as too soon and the retry puts it back.
const UNDO_AT_ONCE: &[(&str, Expect)] = &[
    (
        "turn on round robin scheduling",
        Exact("ok Done: policy round-robin (was interactive). undo"),
    ),
    ("undo", Exact("ok Undone: policy back to interactive")),
];

/// The undo is an entry of its own, so `undo` again redoes.
const REDO: &[(&str, Expect)] = &[("undo", Exact("ok Undone: policy back to round-robin"))];

/// A typed switch moves the policy; `undo` then asks rather than overwrite it,
/// and a no leaves the typed switch alone (compare-and-set).
const MOVED: &[(&str, Expect)] = &[("policy interactive", StartsWith("ok "))];

const MOVED_UNDO: &[(&str, Expect)] = &[
    (
        "undo",
        Exact("ok The policy changed since (now interactive); put interactive back anyway? y/n"),
    ),
    ("n", Exact("ok Not undone; `undo` asks again")),
];

/// A script that switches and then throws leaves nothing changed.
const ROLLBACK: &[(&str, Expect)] = &[
    (
        "switch to round robin then stop",
        StartsWith("err Nothing changed: Runtime error: stopped after the switch"),
    ),
    ("undo", Exact("err nothing to undo")),
];

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
/// switch.
fn after_switch() {
    threads::sleep_ms(MIN_POLICY_INTERVAL_NS / 1_000_000 + 10);
}

pub fn check(which: ActCase) -> Result<(), &'static str> {
    start()?;
    match which {
        ActCase::Plain => plain(),
        ActCase::Run => run(),
        ActCase::Never => never(),
        ActCase::Vague => vague(),
        ActCase::Undo => undo(),
        ActCase::Rollback => rollback(),
    }
}

fn plain() -> Result<(), &'static str> {
    expect_lines(PLAIN)?;
    if !policy_is("round-robin") {
        return Err("act-plain: the plain request did not switch the policy");
    }
    after_switch();
    expect_lines(PLAIN_UNDO)?;
    if !policy_is("interactive") {
        return Err("act-plain: undo did not put the policy back");
    }
    log!("act-plain: plain requests became the right calls, and undo put the policy back");
    Ok(())
}

fn run() -> Result<(), &'static str> {
    expect_lines(RUN)?;
    if !policy_is("round-robin") {
        return Err("act-run: the run-tier call did not switch the policy at once");
    }
    log!("act-run: the run tier ran with no prompt and wrote the result line");
    Ok(())
}

fn never() -> Result<(), &'static str> {
    expect_lines(NEVER)?;
    if !policy_is("interactive") {
        return Err("act-never: a refused call changed the policy");
    }
    log!("act-never: never-tier calls were refused and changed nothing");
    Ok(())
}

fn vague() -> Result<(), &'static str> {
    expect_lines(VAGUE)?;
    if !policy_is("round-robin") {
        return Err("act-vague: the setup switch did not happen");
    }
    after_switch();
    expect_lines(VAGUE_READING)?;
    if !policy_is("interactive") {
        return Err("act-vague: the vague line's reading did not run");
    }
    after_switch();
    expect_lines(NOT_ENGLISH)?;
    if !policy_is("interactive") {
        return Err("act-vague: the line not in English changed the policy without a yes");
    }
    log!("act-vague: a vague line ran as Taken as, and a line not in English asked");
    Ok(())
}

fn undo() -> Result<(), &'static str> {
    expect_lines(UNDO_AT_ONCE)?;
    if !policy_is("interactive") {
        return Err("act-undo: undo inside the rate limit did not put the policy back");
    }
    after_switch();
    expect_lines(REDO)?;
    if !policy_is("round-robin") {
        return Err("act-undo: undo again did not redo the switch");
    }
    after_switch();
    expect_lines(MOVED)?;
    if !policy_is("interactive") {
        return Err("act-undo: the typed switch did not happen");
    }
    after_switch();
    expect_lines(MOVED_UNDO)?;
    if !policy_is("interactive") {
        return Err("act-undo: undo overwrote the typed switch");
    }
    log!("act-undo: undo put back, undo again redid, and a typed switch was not overwritten");
    Ok(())
}

fn rollback() -> Result<(), &'static str> {
    expect_lines(ROLLBACK)?;
    if !policy_is("interactive") {
        return Err("act-rollback: the failed script left its switch in place");
    }
    log!("act-rollback: a script that switched and then threw left nothing changed");
    Ok(())
}
