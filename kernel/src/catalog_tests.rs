//! `test=desk-catalog` (P4 task 3): the Apps sheet. `pkg` runs `pkg-install`'s
//! first step (big from the mirror, beside the image's notes), `desktop` has a
//! `uses=pkg` route, and the shell takes the panel's lines. The case types
//! Ctrl+Space, `apps` and Enter: `desktop` asks `pkg` for the catalogue and
//! lists big and notes installed, then the mirror's `extra`, whose index
//! summary is written as an instruction and is logged as text. Then Down twice
//! and Enter on `extra`: the sheet sends `install extra`, by the row's own
//! name, and the shell takes it as it takes a typed line. The host checks
//! `desktop`'s lines; with no mock model on the command line, any request to
//! the model fails the case.

use crate::display::claim;
use crate::pkg_tests::Step;
use crate::{log, threads};
use alloc::format;
use alloc::string::String;

/// `desktop` with its route to `pkg`, which it asks for the catalogue.
const DESKTOP: &str =
    "desktop caps=display+input+serve+ask+call uses=pkg pages=8192 stack_pages=512";
/// Time for `desktop` to take each step in.
const STEP_MS: u64 = 400;
const CTRL_SPACE: [u8; 4] = [0x1D, 0x39, 0xB9, 0x9D];
const APPS_ENTER: [u8; 10] = [0x1E, 0x9E, 0x19, 0x99, 0x19, 0x99, 0x1F, 0x9F, 0x1C, 0x9C];
const DOWN: [u8; 4] = [0xE0, 0x50, 0xE0, 0xD0];
const ENTER: [u8; 2] = [0x1C, 0x9C];

/// `pkg`'s plan, then `desktop` (planned after `pkg`, its route's target), the
/// model, `cc` (the shell's third route, started on demand) and the shell with
/// its route to `desktop`.
pub fn plan() -> String {
    format!(
        "{}\n{DESKTOP}\n{}\n{}\n{}",
        crate::pkg_tests::plan(Step::Mirror),
        crate::llm::plan_line(),
        kabi::plan::CC,
        kabi::plan::SHELL_DESK
    )
}

pub fn check() -> Result<(), &'static str> {
    crate::pkg_tests::wait_for(&format!("case/{}.done", Step::Mirror.name()))
        .map_err(|_| "desk-catalog: pkg did not finish its install")?;
    crate::desk_tests::wait("desktop to draw", || {
        crate::desk_tests::desktop_shows() && claim::flushes() > 0
    })?;
    crate::desk_tests::wait("the shell", || crate::ipc::server_of("shell").is_some())?;
    crate::claim_tests::type_keys(&CTRL_SPACE);
    threads::sleep_ms(STEP_MS);
    crate::claim_tests::type_keys(&APPS_ENTER);
    threads::sleep_ms(STEP_MS * 3);
    for _ in 0..2 {
        crate::claim_tests::type_keys(&DOWN);
        threads::sleep_ms(STEP_MS);
    }
    crate::claim_tests::type_keys(&ENTER);
    // The shell looks every 250 ms and answers a package line itself.
    threads::sleep_ms(5_000);
    log!("desk-catalog: opened the Apps sheet and pressed Enter on extra");
    Ok(())
}
