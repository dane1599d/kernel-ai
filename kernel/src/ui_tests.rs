//! `test=ui-window` (P3 task 9, ADR 0005 section 4): an app's `kai:ui` window
//! on the desktop. comphost runs `notes-window` with a route to `desktop`; the
//! app opens its window when it starts, comphost lays it out and paints it with
//! the real fonts and sends it over `kai-surface 1`, and `desktop` shows it at
//! the first cascade spot. The case then clicks the window's close control:
//! `desktop` asks, the app hears `close-requested` through `on-ui` and closes
//! the window, and comphost ends its toplevel. Each step shows in comphost's
//! and `desktop`'s log lines.

use crate::display::claim;
use crate::pointer::{self, Event, Motion, Source};
use crate::{console, log, process, threads};

/// The plan lines this case adds.
pub const PLAN: &str = "desktop caps=display+input+serve pages=8192\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1";
const WAIT_MS: u64 = 60_000;
/// Time for `desktop` to take each step in.
const STEP_MS: u64 = 400;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// The window's close control: its 640x420 content in a 642x454 frame at the
/// first cascade spot, 48,52, the control 17 px in from the frame's right end
/// and down from its top, as `desk-windows` finds Left's.
const CLOSE: (i32, i32) = (48 + 642 - 17, 52 + 17);

/// A USB-style relative report with `buttons` held.
fn report(dx: i32, dy: i32, buttons: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        pointer::push(Event {
            source: Source::UsbMouse,
            motion: Motion::Relative { dx, dy },
            buttons,
        });
        crate::input::wake_pending();
    });
    threads::sleep_ms(STEP_MS);
}

pub fn check() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && !process::named("notes-window").is_empty() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        log!("ui-window: the desktop never took the screen");
        return Err("ui-window: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    report(CLOSE.0 - START.0, CLOSE.1 - START.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
    threads::sleep_ms(2_000);
    log!("ui-window: Notes opened its window and pressed its close control");
    Ok(())
}
