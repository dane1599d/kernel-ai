//! `test=app-settings` (P4 task 7's exit case, ADR 0005 §6): the Settings app
//! runs on the desktop as the system image plans it, reads the system's
//! settings through the shell, and its window is kept from the agent. Lines are
//! typed in `term`'s window, as `desk-agent` does, and the mock model answers
//! each by its line (`xtask/src/mock_llm.rs`).
//!
//! "open settings" runs `ui_open`, which starts `settings-window`, the plan
//! line whose name is the app's `kai:settings` grant, through the supervisor's
//! `app-call`. Once it runs, the app's reads go through comphost and `desktop`
//! to the shell and come back (comphost says so once in its log). "look at
//! settings" calls `ui_view`, which answers only the window's title and
//! "private", and the model's `ui_act` on a node it never saw reaches the card
//! and, after a yes, is refused by `desktop`. Each step shows in `desktop`'s and
//! comphost's log lines, which the host checks.

use crate::agent_desk_tests::{KEYS_MS, OPEN_MS, START, click, terminal_keys, wait};
use crate::display::claim;
use crate::{ask_box, log, process, terminal, threads};

/// `desktop`'s shell with a state directory as well (`SHELL_DESK_FILES`), planned
/// after the `fs` service the case's data disk starts: the theme the case sets is
/// saved there, and the shell answers Settings' `file appearance` from it.
pub const SHELL: &str = kabi::plan::SHELL_DESK_FILES;

/// The fixture's title bar: its frame is the second, at 80,84, below the
/// Terminal's, and the click lands 8 px down and 60 px across.
const FIXTURE_TITLE: (i32, i32) = (140, 92);
/// Settings' title bar: its frame is the third, at 112,116. The click is 500 px
/// across, past the fixture's frame (80..402), so neither window can cover the
/// other's title bar however they are raised.
const SETTINGS_TITLE: (i32, i32) = (500, 124);

/// The terminal's window, the fixture's (the test app whose `fs_list` row
/// Settings revokes, never shipped) and Settings', the last two as apps that
/// start only when `ui_open` asks for them, the fixture first. The name is the
/// grant: `desktop` and the shell check it again.
pub const APPS: &str = "term uses=desktop caps=call+term pages=4096 stack_pages=512\n\
     fixture-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand\n\
     settings-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand";

/// A click on `title` gives that window the keys; then the harness sends one
/// stage of keys, and the effects of those keys have `settle_ms` to show.
fn keys_in(case: &str, at: &mut (i32, i32), title: (i32, i32), settle_ms: u64) {
    click(at, title);
    threads::sleep_ms(1_000);
    log!("{case}: ready for keys");
    threads::sleep_ms(settle_ms);
}

pub fn check() -> Result<(), &'static str> {
    const CASE: &str = "app-settings";
    let mut at = START;
    threads::spawn("terminal", terminal::run, 0)?;
    wait(CASE, "desktop and term", || {
        claim::showing() && !process::named("term").is_empty()
    })?;
    // term opens its window with its first call.
    threads::sleep_ms(3_000);
    if !process::named("settings-window").is_empty() {
        return Err("settings-window ran before ui_open asked for it");
    }
    // "switch to the light theme": `appearance_set` runs at once and the shell
    // saves it, before Settings is there to read it.
    terminal_keys(CASE, &mut at)?;
    threads::sleep_ms(KEYS_MS);
    log!("{CASE}: the theme was set before Settings opened");
    // "rules_add on time(22:00) -> app_suspend(game)": a standing instruction,
    // asked on `desktop`'s card; `y` allows it. Settings lists it when it starts.
    terminal_keys(CASE, &mut at)?;
    wait(CASE, "the ask on desktop's card", ask_box::shown)?;
    threads::sleep_ms(KEYS_MS);
    // "y" on the card.
    log!("{CASE}: ready for keys");
    wait(CASE, "the card's answer", || !ask_box::shown())?;
    threads::sleep_ms(KEYS_MS);
    log!("{CASE}: a standing rule was added before Settings opened");
    // "open fixture": the test app, first, so its frame is the second.
    terminal_keys(CASE, &mut at)?;
    wait(CASE, "ui_open to start the fixture", || {
        !process::named("fixture-window").is_empty()
    })?;
    threads::sleep_ms(OPEN_MS / 2);
    log!("{CASE}: ui_open started the fixture and its window opened");
    // "open settings"
    terminal_keys(CASE, &mut at)?;
    wait(CASE, "ui_open to start Settings", || {
        !process::named("settings-window").is_empty()
    })?;
    // The app opens its window, reads its pages and gets its answers.
    threads::sleep_ms(OPEN_MS);
    log!("{CASE}: ui_open started Settings and its window opened");
    // A person's keys in two windows, a gesture the OS can tell from the
    // agent's: a key in the fixture queues its one `fs_list` and the OS runs it;
    // in Settings, `tab right right tab ret` removes the rule from the Rules
    // page; `tab left tab down tab ret` revokes that row and the next key in the
    // fixture is refused; `tab tab tab ret` restores it and the one after runs;
    // `tab left tab ret` presses the Theme button; and `tab tab tab right right
    // right right tab spc` goes round to the Updates page and its switch, which
    // turns the restart window on. Each change is read back from the shell
    // (`apps/settings-ui/tests/keys.rs` pins the sequences).
    const SETTLE_MS: u64 = 2 * KEYS_MS;
    keys_in(CASE, &mut at, FIXTURE_TITLE, SETTLE_MS);
    keys_in(CASE, &mut at, SETTINGS_TITLE, SETTLE_MS);
    keys_in(CASE, &mut at, SETTINGS_TITLE, SETTLE_MS);
    keys_in(CASE, &mut at, FIXTURE_TITLE, SETTLE_MS);
    keys_in(CASE, &mut at, SETTINGS_TITLE, SETTLE_MS);
    keys_in(CASE, &mut at, FIXTURE_TITLE, SETTLE_MS);
    keys_in(CASE, &mut at, SETTINGS_TITLE, SETTLE_MS);
    keys_in(CASE, &mut at, SETTINGS_TITLE, SETTLE_MS);
    log!("{CASE}: keys were pressed in Settings' window");
    // "look at settings": a view of a private window, then an act that asks.
    terminal_keys(CASE, &mut at)?;
    wait(CASE, "the ask on desktop's card", ask_box::shown)?;
    threads::sleep_ms(KEYS_MS);
    // "y" on the card.
    log!("{CASE}: ready for keys");
    wait(CASE, "the card's answer", || !ask_box::shown())?;
    threads::sleep_ms(KEYS_MS);
    log!(
        "app-settings: the agent looked at Settings, saw only that it is private, and could not act in it"
    );
    Ok(())
}
