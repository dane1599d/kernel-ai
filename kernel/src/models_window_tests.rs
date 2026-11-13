//! `test=ui-models-use` (#556): a person's keys in the Models window switch the
//! model, through the shell's `model_use` row, and `undo` switches it back.
//!
//! The store holds xtask's `fixture` as the model in use and the import disk
//! `fixture-2`, the role's other target. The case types `models import` at the
//! shell as a person would at the console, and allows it on the trusted path,
//! so the store holds both. Then the harness's keys go to the window: a click
//! on its title bar gives it the keys, `tab ret` presses Refresh, so it shows
//! fixture-2 stored, and `tab down down tab ret` chooses fixture-2 and presses
//! Use (`apps/models-ui/tests/keys.rs` pins both). The window's `use-model`
//! goes through comphost and `desktop` to the shell, which runs `model_use`
//! with the click's consent; the shell's log shows it, and `undo` typed at the
//! shell puts fixture back for the next restart.

use crate::agent_desk_tests::{KEYS_MS, START, click, wait};
use crate::display::claim;
use crate::{allow, ipc, log, process, threads};
use alloc::string::String;
use termcore::menu::{self, Answer};

const CASE: &str = "ui-models-use";
/// `desktop`, planned before the shell, whose route to it the window's
/// switches come back through.
pub const DESKTOP: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512";
/// The window's title bar: its frame is the first, at 48,52, and the click
/// lands 8 px down and 60 px across.
const TITLE: (i32, i32) = (108, 60);
const CALL_MS: u64 = 60_000;
/// How long `updated` may take to list once it is up, and then to store the
/// import, under TCG.
const WAIT_MS: u64 = 120_000;
const POLL_MS: u64 = 1_000;
/// Time for one stage of keys and what they do: a list read again, or a
/// switch through the shell.
const SETTLE_MS: u64 = 2 * KEYS_MS;

fn shell(line: &str) -> Result<String, &'static str> {
    let reply = ipc::call("shell", line.as_bytes(), CALL_MS)
        .map_err(|_| "ui-models-use: the shell did not reply")?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// The model list, typed as `models`, once it shows `want`.
fn list_until(want: &str) -> Result<String, &'static str> {
    let mut last = String::new();
    for _ in 0..WAIT_MS / POLL_MS {
        last = shell("models")?;
        if last.starts_with("ok ") && last.contains(want) {
            return Ok(last);
        }
        threads::sleep_ms(POLL_MS);
    }
    log!("{CASE}: the list never showed {want:?}: {last:?}");
    Err("ui-models-use: the model list did not show what it should")
}

/// `models import`, which asks; the trusted path allows it, and the shell's
/// reply to the Allow.
fn import_typed() -> Result<String, &'static str> {
    let reply = shell("models import")?;
    let menu = menu::Menu::decode(&reply).map_err(|_| {
        log!("{CASE}: models import answered {reply:?}, not a menu");
        "ui-models-use: the import did not ask first"
    })?;
    let allowed = allow::allow(menu.id, menu.call(), Answer::Allow)
        .ok()
        .flatten()
        .ok_or("ui-models-use: the trusted path could not mint a ONCE")?;
    let reply = shell(&menu::granted_line(menu.id, Answer::Allow, allowed.granted));
    allow::end(&allowed);
    reply
}

/// One stage of keys to the window that has them: the harness sends them.
fn keys() {
    threads::sleep_ms(1_000);
    log!("{CASE}: ready for keys");
    threads::sleep_ms(SETTLE_MS);
}

pub fn check() -> Result<(), &'static str> {
    wait(CASE, "the desktop and the Models window", || {
        claim::showing() && !process::named("models-window").is_empty()
    })?;
    let listed = list_until("(fixture): in use")?;
    if !listed.contains("(fixture-2): signed, not stored") {
        log!("{CASE}: the models: {listed:?}");
        return Err("ui-models-use: the role's other fixture was not listed as signed");
    }
    let imported = import_typed()?;
    if !imported.starts_with("ok Importing fixture-2 ") {
        log!("{CASE}: the Allow answered {imported:?}");
        return Err("ui-models-use: the allowed import did not start");
    }
    list_until("(fixture-2): stored")?;
    log!("{CASE}: fixture-2 was imported at the shell, as a person types it");

    // The window has the keys: Refresh, then fixture-2 and Use.
    let mut at = START;
    click(&mut at, TITLE);
    keys();
    keys();
    list_until("(fixture-2): from the next restart")?;
    let logged = shell("log actlog")?;
    if !logged.contains("consent=user-click:models") {
        log!("{CASE}: the log: {logged:?}");
        return Err("ui-models-use: the switch was not logged as the person's click");
    }
    log!("{CASE}: a key in the window switched the model, logged as the person's click");

    let undone = shell("undo")?;
    if undone != "ok Undone: the model before, from the next restart" {
        log!("{CASE}: undo answered {undone:?}");
        return Err("ui-models-use: undo took back something else");
    }
    list_until("(fixture-2): stored")?;
    log!(
        "ui-models-use: a person's keys in the Models window switched the model through the shell, and undo switched it back"
    );
    Ok(())
}
