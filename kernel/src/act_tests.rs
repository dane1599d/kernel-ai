//! P1 task 7's boot cases (`test=act-plain`, `act-run`, `act-never`,
//! `act-vague`): one tagged call through the one door. The shell sends `act` to
//! the llm service, which asks the mock LLM server; its calls are keyed by the
//! request line (`ACTS` in xtask's `mock_llm`). Code, not the model, decides what
//! each call may do, and writes the reply.

use crate::sched_control::calls::MIN_POLICY_INTERVAL_NS;
use crate::selftest::ActCase;
use crate::shell_tests::Expect::{self, Contains, Exact, StartsWith};
use crate::shell_tests::{expect_lines, start};
use crate::{allow, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use termcore::menu::{self, Answer, Granted};

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
        // The Allow / Deny menu, highlight on Deny, with no rule offered.
        Exact(
            "menu 3 added\nRun set_policy(\"round-robin\")? The request is not in English, so it asks first.\n\n\nset_policy(\"round-robin\")",
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
        ActCase::Ask => ask(),
        ActCase::Edit => edit(),
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

/// The shell's raw reply to `line`.
fn reply_to(line: &str) -> Result<String, &'static str> {
    let reply = ipc::call("shell", line.as_bytes(), 30_000).map_err(|_| "act-ask: no reply")?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// Asks the not-English switch, which must come back as the Allow / Deny menu;
/// its log entry id and the call as the menu shows it.
fn asked() -> Result<(u64, String), &'static str> {
    let reply = reply_to("cambia la política a round robin")?;
    let menu = menu::Menu::decode(&reply).map_err(|_| {
        log!("act-ask: not a menu: {reply:?}");
        "act-ask: an asking call did not get the menu"
    })?;
    Ok((menu.id, String::from(menu.call())))
}

/// The answer to an asked call must say it did not run, with `why`.
fn refused(line: &str, why: &str) -> Result<(), &'static str> {
    let reply = reply_to(line)?;
    if !(reply.starts_with("ok script ") && reply.contains("not run") && reply.contains(why)) {
        log!("act-ask: {line:?} answered {reply:?}");
        return Err("act-ask: an Allow that should not run did");
    }
    Ok(())
}

/// The Allow / Deny menu on handles (ADR 0001 D6, ADR 0002 D2a): the trusted
/// path's Allow mints a ONCE whose kernel-held digest names the call as
/// shown, and the call runs only when the shell's ONCE_CHECK matches, once.
fn ask() -> Result<(), &'static str> {
    // No ONCE, a typed yes, or a Deny: nothing runs.
    let (id, _) = asked()?;
    refused(&format!("answer {id} allow"), "without its ONCE")?;
    let (id, _) = asked()?;
    refused("y", "answered on the menu")?;
    let (id2, _) = asked()?;
    if id2 == id || reply_to(&format!("answer {id2} deny"))? != format!("ok script {id2} not run") {
        return Err("act-ask: Deny did not decline");
    }
    if !policy_is("interactive") {
        return Err("act-ask: a call ran without its Allow");
    }
    // Allow: the trusted path mints, the shell checks, the call runs.
    let (id, question) = asked()?;
    let allowed = allow::allow(id, &question, Answer::Allow)
        .ok()
        .flatten()
        .ok_or("act-ask: the trusted path could not mint a ONCE")?;
    let reply = reply_to(&menu::granted_line(id, Answer::Allow, allowed.granted))?;
    allow::end(&allowed);
    if reply != "ok Done: policy round-robin (was interactive). undo" || !policy_is("round-robin") {
        log!("act-ask: the Allow answered {reply:?}");
        return Err("act-ask: an Allow with its ONCE did not run");
    }
    // The same ONCE again, after its request ended: refused.
    let (replay, _) = asked()?;
    let line = menu::granted_line(replay, Answer::Allow, allowed.granted);
    refused(&line, "did not check")?;
    // A ONCE minted for other arguments: its digest differs, so refused.
    let (id, question) = asked()?;
    let other = question.replace("round-robin", "interactive");
    let wrong = allow::allow(id, &other, Answer::Allow)
        .ok()
        .flatten()
        .ok_or("act-ask: the trusted path could not mint a ONCE")?;
    let line = menu::granted_line(
        id,
        Answer::Allow,
        Granted {
            rule: None,
            ..wrong.granted
        },
    );
    refused(&line, "did not check (Invalid)")?;
    allow::end(&wrong);
    log!(
        "act-ask: the menu's Allow ran a call once through its ONCE; none, a typed y, a reused one and one for other arguments did not"
    );
    Ok(())
}

/// `act-edit`: a tagged call opens the editor at a line through `do`, which the
/// terminal parses itself and runs as `:edit` would; `do` lines it must refuse
/// change nothing.
fn edit() -> Result<(), &'static str> {
    for bad in [
        "do 90 rm notes",
        "do 91 edit notes ../secrets 1",
        "do 92 edit notes src/nowhere.rs 1",
        "do 93 edit notes src/lib.rs 99999",
        "do 94 edit ghost src/lib.rs 1",
        "do 95 edit notes src/lib.rs 3 :quit",
    ] {
        let done = crate::doing::run(bad);
        if !done.line.starts_with("done ") || !done.line.contains(" err ") || done.apply.is_some() {
            log!("act-edit: {bad:?} gave {:?}", done.line);
            return Err("act-edit: a do the terminal must refuse ran");
        }
    }
    if crate::edit_mode::editing().is_some() {
        return Err("act-edit: a refused do opened the editor");
    }
    let reply = reply_to("open notes/src/lib.rs at line 3")?;
    if !reply.starts_with("do ") || !reply.ends_with(" edit notes src/lib.rs 3") {
        log!("act-edit: the call answered {reply:?}");
        return Err("act-edit: the editor call did not become a do");
    }
    let done = crate::doing::run(&reply);
    if !done.line.contains(" ok ") {
        log!("act-edit: the do gave {:?}", done.line);
        return Err("act-edit: the terminal did not open the editor");
    }
    let result = reply_to(&done.line)?;
    if !result.starts_with("ok Done: ") || crate::edit_mode::editing().as_deref() != Some("notes") {
        log!("act-edit: the done answered {result:?}");
        return Err("act-edit: the editor is not open on notes");
    }
    log!(
        "act-edit: a tagged call opened the editor at a line through do; refused do lines changed nothing"
    );
    Ok(())
}
