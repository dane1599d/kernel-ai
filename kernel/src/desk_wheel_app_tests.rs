//! `test=desk-wheel-app` (lane G, ADR 0005 section 3's wheel): the wheel over an
//! app's list scrolls it. Notes' window opens as in `ui-window`; the case adds
//! thirty notes through the app's own `add` (more rows than its list is tall),
//! moves the pointer onto the list, and turns the wheel. `desktop` posts the
//! wheel to the window's event ring ("the wheel went to window 1"); comphost
//! hands it to `uikit`, whose list scrolls and so draws again, which reaches
//! `desktop` as the window's next picture ("window 1 drew after the wheel"):
//! an app that took no scroll would send none. The host checks the lines.

use crate::desk_tests::{report, report_wheel};
use crate::display::claim;
use crate::{ipc, log, process, threads};

/// The plan lines: `ui-window`'s, with `desktop` logging the wheel (`wheel_log=on`).
pub const PLAN: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512 wheel_log=on\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1";
const CASE: &str = "desk-wheel-app";
const WAIT_MS: u64 = 60_000;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
const CALL_MS: u64 = 10_000;
/// Notes added: twice what its list shows at 640x420.
const NOTES: u32 = 30;
/// Where `desktop` starts the pointer: the middle of the screen.
const START: (i32, i32) = (640, 400);
/// On the list in Notes' left column: its window is the first, at 48,52, with
/// its content from 49,85 and the list under the search field and the button.
const LIST: (i32, i32) = (150, 350);

pub fn check() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        if crate::desk_tests::desktop_shows()
            && claim::flushes() > 0
            && !process::named("notes-window").is_empty()
        {
            break;
        }
        threads::sleep_ms(50);
    }
    if !crate::desk_tests::desktop_shows() {
        log!("{CASE}: the desktop never took the screen");
        return Err("desk-wheel-app: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    for n in 0..NOTES {
        let add = alloc::format!(
            "kai-call 1
func app:notes/actions@1.0.0#add
args {{text: \"note {n}\"}}
"
        );
        if ipc::call("notes-window", add.as_bytes(), CALL_MS).is_err() {
            log!("{CASE}: Notes did not take note {n}");
            return Err("desk-wheel-app: Notes did not take a note");
        }
    }
    log!("{CASE}: Notes holds {NOTES} notes");
    threads::sleep_ms(3_000);
    // The pointer onto the list, then one detent toward the person.
    report(LIST.0 - START.0, LIST.1 - START.1, 0);
    threads::sleep_ms(1_000);
    report_wheel(0, 0, 0, 1);
    threads::sleep_ms(3_000);
    log!("{CASE}: the wheel was turned over Notes' list");
    Ok(())
}
