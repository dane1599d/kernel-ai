//! `test=desk-default`: the v0.1 system as the default boot plans it
//! (`kabi::plan::system`, no plan line of the case's own), on a disk with
//! `kai-data`. `desktop` claims the screen and draws; `pkg` seeds the image's
//! signed repository and serves its catalogue; the case types Ctrl+Space,
//! `apps` and Enter, and the Apps sheet lists the catalogue with `notes`
//! installed first. Enter on it sends `open notes` as a panel line, the shell
//! takes it, the model's `ui_open` starts `notes-window` on demand, and its
//! window opens. The host checks `desktop`'s and the supervisor's lines.

use crate::display::claim;
use crate::{desk_tests, ipc, log, process, threads};

/// Time for `desktop` to take each step in.
const STEP_MS: u64 = 400;
/// Ctrl+Space; `apps` and Enter; Enter (scan code set 1).
const CTRL_SPACE: [u8; 4] = [0x1D, 0x39, 0xB9, 0x9D];
const APPS_ENTER: [u8; 10] = [0x1E, 0x9E, 0x19, 0x99, 0x19, 0x99, 0x1F, 0x9F, 0x1C, 0x9C];
const ENTER: [u8; 2] = [0x1C, 0x9C];
/// The app the sheet's first row opens.
const APP: &str = "notes-window";
/// `pkg` answers once it has seeded the store, so its first answer can wait on
/// that under TCG.
const CALL_MS: u64 = 60_000;

pub fn check() -> Result<(), &'static str> {
    desk_tests::wait("desktop to draw", || {
        desk_tests::desktop_shows() && claim::flushes() > 0
    })?;
    desk_tests::wait("the shell", || ipc::server_of("shell").is_some())?;
    // `pkg` serves only once the image's repository is generation 1.
    if ipc::call("pkg", b"catalog 0", CALL_MS).is_err() {
        return Err("desk-default: pkg did not answer");
    }
    if !process::named(APP).is_empty() {
        return Err("desk-default: an app started before it was opened");
    }
    crate::claim_tests::type_keys(&CTRL_SPACE);
    threads::sleep_ms(STEP_MS);
    crate::claim_tests::type_keys(&APPS_ENTER);
    threads::sleep_ms(STEP_MS * 3);
    crate::claim_tests::type_keys(&ENTER);
    desk_tests::wait("the app to start", || !process::named(APP).is_empty())?;
    // comphost starts the app and paints its first frame, slowly under TCG.
    threads::sleep_ms(10_000);
    log!(
        "desk-default: the desktop came up, the Apps sheet listed the catalogue, and Enter opened notes-window"
    );
    Ok(())
}
