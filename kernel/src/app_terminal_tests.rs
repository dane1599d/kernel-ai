//! `test=app-terminal` (P4 task 4's exit case): the AI types a command into the
//! Terminal app through its action, and only a person's key runs it. The
//! mock model answers "type 1 + 2 in the terminal" with `terminal_window_run`,
//! which the shell loads from the app on first use and calls through the
//! supervisor; the line waits in the app's input and nothing ran. Then the
//! harness presses Enter, which `desktop` posts to the window, comphost hands
//! to `uikit`, and the app turns into a `shell_line` effect marked as a
//! person's: `desktop` queues it, the shell runs it in the app's own terminal
//! and the answer comes back as a row. The AI reads the rows with
//! `terminal_window_read`.

use crate::display::claim;
use crate::shell_tests::Expect::{Contains, StartsWith};
use crate::shell_tests::expect_lines;
use crate::{console, ipc, log, process, threads};
use alloc::string::String;

const NAME: &str = "terminal-window";
/// The plan lines this case adds, after the shell's `cc` and before the shell,
/// which has a route to `desktop` (`SHELL_DESK`).
pub const PLAN: &str = "desktop caps=display+input+serve+ask pages=8192 stack_pages=512\n\
     terminal-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1";
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// How long the AI's typed line may sit before the case checks that it did not run.
const SETTLE_MS: u64 = 2_000;
/// How long Enter may take to become a reply row.
const REPLY_MS: u64 = 60_000;
const READ: &str = "kai-call 1\nfunc app:terminal-window/actions@1.0.0#read\nargs {since: 0}\n";

/// What the app's rows say now, as the app answers its `read` action.
fn rows() -> Result<String, &'static str> {
    ipc::call(NAME, READ.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-terminal: terminal-window did not reply")
}

pub fn check() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && !process::named(NAME).is_empty() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        return Err("app-terminal: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);

    // The AI types the line; the app answers at once.
    expect_lines(&[("type 1 + 2 in the terminal", StartsWith("ok "))])?;
    threads::sleep_ms(SETTLE_MS);
    let typed = rows()?;
    log!(
        "app-terminal: after the AI typed, the rows are {}",
        typed.replace('\n', "; ")
    );
    if !typed.contains("The AI typed a line for you") {
        return Err("app-terminal: the typed line was not announced");
    }
    if typed.contains(" reply ") || typed.contains("input kai> 1 + 2") {
        return Err("app-terminal: the AI's line ran without a key");
    }

    // A person presses Enter.
    log!("app-terminal: ready for keys");
    let mut ran = String::new();
    for _ in 0..REPLY_MS / 200 {
        threads::sleep_ms(200);
        ran = rows()?;
        if ran.contains("reply 3") {
            break;
        }
    }
    log!(
        "app-terminal: after Enter, the rows are {}",
        ran.replace('\n', "; ")
    );
    if !ran.contains("input kai> 1 + 2") || !ran.contains("reply 3") {
        return Err("app-terminal: Enter did not run the typed line");
    }

    // The AI reads what the terminal shows.
    expect_lines(&[("show what the terminal says", Contains("reply 3"))])?;
    log!("app-terminal: the AI typed a line, a person's Enter ran it, and the AI read its reply");
    Ok(())
}
