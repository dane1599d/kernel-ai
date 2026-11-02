//! `test=app-models` (P4 task 10's exit case): the agent manages the models
//! through the `model_*` rows, which `updated` carries out over the shell's
//! route to it. The store holds xtask's `fixture` as the model in use
//! (`store:`), and the import disk holds `fixture-2`, the image's `models`
//! role's other test target. The mock model answers each line with its row
//! (`ACTS` in xtask's `mock_llm`):
//!
//! - "list the AI models" (`model_list`) shows `fixture` in use and
//!   `fixture-2` signed but not stored;
//! - "import the model on the attached disk" (`model_fetch`) asks once, as a
//!   row with no undo does; the trusted path allows it, and `updated` stores
//!   the disk's bytes only once their length and SHA-256 are the target's;
//! - "use the fixture-2 model" (`model_use`) runs, since the line names it:
//!   the next restart uses it, on trial;
//! - `undo` puts `fixture` back for the next restart.

use crate::shell_tests::Expect::{Contains, Exact, StartsWith};
use crate::shell_tests::expect_lines;
use crate::{allow, ipc, log, threads};
use alloc::string::String;
use termcore::menu::{self, Answer};

const LIST: &str = "list the AI models";
const IMPORT: &str = "import the model on the attached disk";
const CALL_MS: u64 = 60_000;
/// How long `updated` may take to list once it is up, and then to store the
/// import, under TCG.
const WAIT_MS: u64 = 120_000;
const POLL_MS: u64 = 1_000;

fn reply_to(line: &str) -> Result<String, &'static str> {
    let reply = ipc::call("shell", line.as_bytes(), CALL_MS)
        .map_err(|_| "app-models: the shell did not reply")?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// The model list once it shows `want`, asked again each [`POLL_MS`].
fn list_until(want: &str) -> Result<String, &'static str> {
    let mut last = String::new();
    for _ in 0..WAIT_MS / POLL_MS {
        last = reply_to(LIST)?;
        if last.starts_with("ok ") && last.contains(want) {
            return Ok(last);
        }
        threads::sleep_ms(POLL_MS);
    }
    log!("app-models: the list never showed {want:?}: {last:?}");
    Err("app-models: the model list did not show what it should")
}

/// The AI asks to import: the shell must come back with its menu, which the
/// trusted path then allows, and the shell's reply to the Allow.
fn import_after_allow() -> Result<String, &'static str> {
    let reply = reply_to(IMPORT)?;
    let menu = menu::Menu::decode(&reply).map_err(|_| {
        log!("app-models: {IMPORT:?} answered {reply:?}, not a menu");
        "app-models: the import did not ask first"
    })?;
    if !menu.call().contains("model_fetch") {
        log!("app-models: the menu showed {:?}", menu.call());
        return Err("app-models: the menu did not show the import");
    }
    let allowed = allow::allow(menu.id, menu.call(), Answer::Allow)
        .ok()
        .flatten()
        .ok_or("app-models: the trusted path could not mint a ONCE")?;
    let reply = reply_to(&menu::granted_line(menu.id, Answer::Allow, allowed.granted));
    allow::end(&allowed);
    reply
}

pub fn check() -> Result<(), &'static str> {
    let listed = list_until("(fixture): in use")?;
    log!("app-models: the models: {}", listed.replace('\n', "; "));
    if !listed.contains("(fixture-2): signed, not stored") {
        return Err("app-models: the role's other fixture was not listed as signed");
    }

    let imported = import_after_allow()?;
    if !imported.starts_with("ok Importing fixture-2 ") {
        log!("app-models: the Allow answered {imported:?}");
        return Err("app-models: the allowed import did not start");
    }
    list_until("(fixture-2): stored")?;

    expect_lines(&[
        (
            "use the fixture-2 model",
            StartsWith("ok Done: the next restart uses it, on trial"),
        ),
        (LIST, Contains("(fixture-2): from the next restart")),
        (
            "undo",
            Exact("ok Undone: the model before, from the next restart"),
        ),
        // `fixture` runs throughout (no restart); the undo shows in `fixture-2`
        // going back from next to stored.
        (LIST, Contains("(fixture-2): stored")),
    ])?;
    log!(
        "app-models: the agent listed the models, imported fixture-2 checked against the signed role, used it, and undo put fixture back"
    );
    Ok(())
}
