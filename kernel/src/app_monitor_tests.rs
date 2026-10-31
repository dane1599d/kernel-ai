//! `test=app-monitor` (P4 task 9's exit case): the System monitor runs on the
//! desktop as the plan line has it, and the agent works it through the shell.
//! The mock model answers "sort the monitor by memory" with `monitor_sort`, which
//! the shell loads from the app on first use and calls through the supervisor;
//! the app answers from the sample comphost gave it, naming the top process. "select
//! spin-a in the monitor" finds an unsupervised spinner the plan adds, in that
//! same sample. "stop spin-a" is the OS's `process_stop`, which asks: the case
//! reads the menu, answers Deny, and the spinner is still running.

use crate::display::claim;
use crate::{console, ipc, log, process, threads};
use alloc::format;
use alloc::string::String;

const NAME: &str = "monitor-window";
/// The plan lines this case adds, after the shell's `cc` and before the shell,
/// which has a route to `desktop` (`SHELL_DESK`): the Monitor's own line, and a
/// spinner for it to list and for a stop to ask about.
pub fn plan() -> String {
    format!(
        "desktop caps=display+input+serve+ask pages=8192 stack_pages=512\n{}\n\
         spin-a program=spinner burst_ms=3600000 unsupervised",
        kabi::plan::MONITOR_WINDOW
    )
}

const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app, paint its first frame and give it a few
/// samples under TCG.
const OPEN_MS: u64 = 12_000;
/// The requests the mock model answers (`xtask/src/mock_llm.rs`).
const SORT: &str = "sort the monitor by memory";
const SELECT: &str = "select spin-a in the monitor";
const STOP: &str = "stop spin-a";

/// The shell's raw reply to `line`.
fn reply_to(line: &str) -> Result<String, &'static str> {
    let reply = ipc::call("shell", line.as_bytes(), CALL_MS)
        .map_err(|_| "app-monitor: the shell did not reply")?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
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
        return Err("app-monitor: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);

    // The agent sorts by memory; the app names the top process from its sample.
    let sorted = reply_to(SORT)?;
    log!(
        "app-monitor: {SORT:?} answered {:?}",
        sorted.replace('\n', "; ")
    );
    if sorted.contains("No processes are listed yet") {
        return Err("app-monitor: the Monitor had no sample to sort");
    }
    if !(sorted.starts_with("ok ") && sorted.contains("Sorted by memory. First: ")) {
        return Err("app-monitor: the sort did not name the top process");
    }

    // The same sample lists the spinner the plan added.
    let selected = reply_to(SELECT)?;
    log!(
        "app-monitor: {SELECT:?} answered {:?}",
        selected.replace('\n', "; ")
    );
    if !(selected.starts_with("ok ") && selected.contains("Selected spin-a")) {
        return Err("app-monitor: the Monitor did not list the spinner");
    }

    // Stopping a process asks. A Deny leaves it running.
    let asked = reply_to(STOP)?;
    let menu = termcore::menu::Menu::decode(&asked).map_err(|_| {
        log!("app-monitor: {STOP:?} answered {asked:?}, not a menu");
        "app-monitor: a stop did not ask with the menu"
    })?;
    if !menu.question().starts_with("Stop process spin-a (pid ") {
        log!("app-monitor: the question was {:?}", menu.question());
        return Err("app-monitor: the menu did not name the process");
    }
    let denied = reply_to(&format!("answer {} deny", menu.id))?;
    if denied != format!("ok script {} not run", menu.id) {
        log!("app-monitor: the Deny answered {denied:?}");
        return Err("app-monitor: Deny did not decline the stop");
    }
    if process::named("spin-a").is_empty() {
        return Err("app-monitor: the spinner stopped without an Allow");
    }
    log!(
        "app-monitor: the agent sorted the Monitor by memory and it named its top process, it found spin-a, and stopping spin-a asked and ended on Deny"
    );
    Ok(())
}
