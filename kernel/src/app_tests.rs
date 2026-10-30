//! The cases for the P4 apps that are components (`test=app-monitor`, and the
//! other apps' as they come): each boots the app's plan line on the desktop,
//! lets it open its window (and, for the Monitor, be given its first sample), and
//! presses the window's close control, as `ui-window` does for Notes. What each
//! step did shows in comphost's and `desktop`'s log lines, which the case in
//! `xtask/src/cases/apps.rs` expects.
//!
//! `app-monitor`: comphost runs `monitor-window` with a route to `desktop` for its
//! window and one to `llm` for `health`; the app, granted `kai:system` by its name
//! in the plan line, opens its window and asks for a sample, and comphost reads
//! `METRICS` and gives it one (it says so once in its log).

use crate::display::claim;
use crate::ui_tests::report;
use crate::{console, log, process, threads};
use alloc::format;
use alloc::string::String;

/// The plan lines `test=app-monitor` adds, after the `llm` line every case has:
/// `desktop` first, as the app's route to it must be listed before it.
pub fn monitor_plan() -> String {
    format!(
        "desktop caps=display+input+serve pages=8192 stack_pages=512\n{}",
        kabi::plan::MONITOR_WINDOW
    )
}

const WAIT_MS: u64 = 60_000;
/// Time for comphost to start the app, paint its first frame and, for the
/// Monitor, give it a sample, under TCG.
const OPEN_MS: u64 = 12_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// An app window's close control: its frame is 642 wide at the first cascade
/// spot, 48,52, the control 17 px in from the frame's right end and down from its
/// top, as `ui-window` finds it for Notes. The width is the same for any app
/// whose content is 640 wide; a wider one has it further right.
fn close_at(content_width: i32) -> (i32, i32) {
    (48 + content_width + 2 - 17, 52 + 17)
}

/// Waits for the desktop to take the screen and the app `process` to run, gives
/// it time to open, and presses the close control of its window, `content_width`
/// px wide. `never` is what fails the case; `done` is what it logs.
fn window_check(
    process_name: &str,
    content_width: i32,
    never: &'static str,
    done: &'static str,
) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && !process::named(process_name).is_empty() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        log!("{never}");
        return Err(never);
    }
    threads::sleep_ms(OPEN_MS);
    let close = close_at(content_width);
    report(close.0 - START.0, close.1 - START.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
    threads::sleep_ms(2_000);
    log!("{done}");
    Ok(())
}

pub fn monitor_check() -> Result<(), &'static str> {
    window_check(
        "monitor-window",
        640,
        "app-monitor: the desktop never took the screen",
        "app-monitor: the Monitor's window was shown, and its close control pressed",
    )
}
