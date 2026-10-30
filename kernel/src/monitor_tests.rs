//! `test=app-monitor` (P4 task 9): the System monitor's window on the desktop.
//! comphost runs `monitor-window` with a route to `desktop` for its window and
//! one to `llm` for `health`; the app, granted `kai:system` by its name in the
//! plan line, opens its window and asks for a sample, comphost reads `METRICS`
//! and gives it one (it says so once in its log), and the case then presses the
//! window's close control, as `ui-window` does for Notes. Each step shows in
//! comphost's and `desktop`'s log lines.

use crate::display::claim;
use crate::ui_tests::report;
use crate::{console, log, process, threads};
use alloc::format;
use alloc::string::String;

/// The plan lines this case adds, after the `llm` line every case has:
/// `desktop` first, as the app's route to it must be listed before it.
pub fn plan() -> String {
    format!(
        "desktop caps=display+input+serve pages=8192 stack_pages=512\n{}",
        kabi::plan::MONITOR_WINDOW
    )
}

const WAIT_MS: u64 = 60_000;
/// Time for comphost to start the app, paint its first frame and give it a
/// sample under TCG.
const OPEN_MS: u64 = 12_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// The window's close control: its 640x440 content in a 642x474 frame at the
/// first cascade spot, 48,52, the control 17 px in from the frame's right end
/// and down from its top, as `ui-window` finds it for Notes.
const CLOSE: (i32, i32) = (48 + 642 - 17, 52 + 17);

pub fn check() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && !process::named("monitor-window").is_empty() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        log!("app-monitor: the desktop never took the screen");
        return Err("app-monitor: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    report(CLOSE.0 - START.0, CLOSE.1 - START.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
    threads::sleep_ms(2_000);
    log!("app-monitor: the Monitor's window was shown, and its close control pressed");
    Ok(())
}
