//! `test=desk-taskbar` (ADR 0005 §8, "The taskbar"): the taskbar by clicks, on
//! the v0.1 system as `desk-start` boots it (the Terminal's window and Notes'
//! window service, which starts only when asked), at 1280x800. Every click is a
//! person's, through the pointer queue; `desktop`'s own lines say what each did,
//! which the host checks (`xtask`'s case):
//! 1. a click on the Notes pin, which has no window, sends `open notes` as the
//!    taskbar's own line, never a request for the model, and Notes opens;
//! 2. a click on its item, its window focused, minimizes it; a second click
//!    restores it;
//! 3. a click on the Kai button opens Start (its sheet lists the apps), a
//!    second closes it;
//! 4. a click on the Kai chip opens the pane; with it open, Notes maximized
//!    stops 12 px left of it, and a second click on the chip closes the pane
//!    and the window fits the whole work area again.
//!
//! The taskbar's items are the Kai button and the pins of the image's five
//! apps, terminal, files, notes, monitor and settings: Notes is item 3 of 6.

use crate::agent_desk_tests::{OPEN_MS, wait};
use crate::desk_tests::click_at;
use crate::ui_tests::{APP_FRAME_W, DESK};
use crate::{input, log, process, terminal, threads};

const CASE: &str = "desk-taskbar";
/// How many items the taskbar holds: the Kai button and five pins.
const ITEMS: i32 = 6;
/// Notes' item: the Kai button, terminal, files, then notes.
const NOTES_ITEM: i32 = 3;
/// Notes' window is the second on the desk (the Terminal's is the first).
const NOTES_SPOT: i32 = 1;
/// Time for `desktop` to take a click in and draw it.
const SETTLE_MS: u64 = 1_500;

pub fn check() -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    wait(CASE, "desktop and term", || {
        crate::display::claim::showing() && !process::named("term").is_empty()
    })?;
    wait(CASE, "the terminal's key lease", input::leased)?;
    if !process::named("notes-window").is_empty() {
        return Err("desk-taskbar: notes-window ran before its pin was clicked");
    }
    let click = |what: &str, at: (i32, i32)| {
        click_at(&DESK, at);
        threads::sleep_ms(SETTLE_MS);
        log!("{CASE}: clicked {what}");
    };
    let notes = DESK.taskbar_item(NOTES_ITEM, ITEMS);
    click("the Notes pin", notes);
    wait(CASE, "the pin to open Notes", || {
        !process::named("notes-window").is_empty()
    })?;
    threads::sleep_ms(OPEN_MS);
    click("Notes' item, its window focused", notes);
    click("Notes' item, its window minimized", notes);
    let kai = DESK.taskbar_item(0, ITEMS);
    click("the Kai button", kai);
    click("the Kai button again", kai);
    click("the Kai chip", DESK.kai_chip());
    click(
        "Notes' maximize control, the pane open",
        DESK.maximize(NOTES_SPOT, APP_FRAME_W),
    );
    click("the Kai chip again", DESK.kai_chip());
    log!("{CASE}: the taskbar opened, minimized, restored and maximized beside the pane");
    Ok(())
}
