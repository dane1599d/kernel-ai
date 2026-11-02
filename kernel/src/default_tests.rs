//! `test=desk-default`: the v0.1 system as the default boot plans it
//! (`kabi::plan::system`, no plan line of the case's own), on a disk with
//! `kai-data`. `desktop` claims the screen and draws; `pkg` seeds the image's
//! signed repository and serves its catalogue; the case types Ctrl+Space,
//! `apps` and Enter, and the Apps sheet lists the catalogue with `notes`
//! installed first. Enter on it sends `open notes` as a panel line, the shell
//! takes it, the model's `ui_open` starts `notes-window` on demand, and its
//! window opens. The host checks `desktop`'s and the supervisor's lines.
//!
//! `test=desk-no-memory`: the same system and the same steps, but first a
//! process takes what is left of the admission budget (free / 2), so the
//! kernel refuses `notes-window`'s start. `open notes`, sent to the shell as
//! the terminal sends a line, goes through the model's `ui_open` as the panel's
//! does; the supervisor answers at once that there is no memory to start it,
//! and the shell's answer, the text the panel shows, is "Not enough memory to
//! open Notes" instead of a silent failure.

use crate::display::claim;
use crate::process::SpawnRequest;
use crate::{desk_tests, ipc, log, process, threads};
use alloc::string::String;
use kabi::{ExitReason, Limits};

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
/// What the shell says when there is no memory to open Notes.
const NOT_ENOUGH: &str = "Not enough memory to open Notes";

/// The desktop drawn, the shell up, and `pkg` seeded.
fn system_up() -> Result<(), &'static str> {
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
    Ok(())
}

/// Opens the Apps sheet from the panel and presses Enter on its first row.
fn open_first_app() {
    crate::claim_tests::type_keys(&CTRL_SPACE);
    threads::sleep_ms(STEP_MS);
    crate::claim_tests::type_keys(&APPS_ENTER);
    threads::sleep_ms(STEP_MS * 3);
    crate::claim_tests::type_keys(&ENTER);
}

pub fn check() -> Result<(), &'static str> {
    system_up()?;
    open_first_app();
    desk_tests::wait("the app to start", || !process::named(APP).is_empty())?;
    // comphost starts the app and paints its first frame, slowly under TCG.
    threads::sleep_ms(10_000);
    log!(
        "desk-default: the desktop came up, the Apps sheet listed the catalogue, and Enter opened notes-window"
    );
    Ok(())
}

/// What the admission budget still has room for.
fn room() -> u64 {
    let committed: u64 = process::with_table(|t| {
        t.iter()
            .filter(|p| p.live())
            .map(|p| p.limits.max_pages)
            .sum()
    });
    process::page_budget().saturating_sub(committed)
}

/// Names for the processes that hold the rest, one quota each at most
/// [`process::MAX_PAGES_PER_PROCESS`].
const HOLDS: [&str; 8] = [
    "memory-hold-1",
    "memory-hold-2",
    "memory-hold-3",
    "memory-hold-4",
    "memory-hold-5",
    "memory-hold-6",
    "memory-hold-7",
    "memory-hold-8",
];
/// Left in the budget: less than one app's window (8192 pages) needs.
const LEFT: u64 = 64;

/// Processes holding what is left of the admission budget, less [`LEFT`].
fn hold_the_rest() -> Result<alloc::vec::Vec<u64>, &'static str> {
    let mut holds = alloc::vec::Vec::new();
    for name in HOLDS {
        let pages = room()
            .saturating_sub(LEFT)
            .min(process::MAX_PAGES_PER_PROCESS);
        if pages < 64 {
            return Ok(holds);
        }
        let pid = process::spawn(&SpawnRequest {
            name,
            program: "handles",
            args: b"mode=idle",
            limits: Limits {
                max_pages: pages,
                max_burst_ms: 30_000,
                caps: 0,
                stack_pages: 0,
            },
            spawner: 0,
            supervisor: false,
            uses: &[],
        });
        match pid {
            Ok(pid) => holds.push(pid),
            Err(error) => {
                log!("desk-no-memory: cannot start {name} with {pages} pages: {error:?}");
                for pid in holds {
                    let _ = process::kill(pid, ExitReason::Killed);
                }
                return Err("desk-no-memory: cannot start the processes that hold the rest");
            }
        }
    }
    match room() < 8192 {
        true => Ok(holds),
        false => Err("desk-no-memory: the budget is too large to hold"),
    }
}

/// The shell's answer to `open notes`, as the terminal gets it: the same reply
/// the panel shows for the line.
fn open_notes() -> Result<String, &'static str> {
    ipc::call("shell", b"open notes", OPEN_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "desk-no-memory: the shell did not answer open notes")
}

/// The supervisor's app-call waits 5 s at most, and the shell's 15 s.
const OPEN_MS: u64 = 60_000;
/// Several times the supervisor's one-second retry of a refused start.
const RETRY_WAIT_MS: u64 = 5_000;

pub fn no_memory_check() -> Result<(), &'static str> {
    system_up()?;
    let holds = hold_the_rest()?;
    let answer = open_notes();
    for pid in holds {
        let _ = process::kill(pid, ExitReason::Killed);
    }
    let answer = answer?;
    if !answer.contains(NOT_ENOUGH) {
        log!("desk-no-memory: open notes answered {answer:?}");
        return Err("desk-no-memory: the shell did not say there is no memory");
    }
    // Memory is free again; the app waits to be asked, and does not open by
    // itself past the supervisor's one-second retry.
    threads::sleep_ms(RETRY_WAIT_MS);
    if !process::named(APP).is_empty() {
        return Err("desk-no-memory: notes-window opened by itself once memory freed");
    }
    log!("desk-no-memory: with no memory left, opening Notes said so plainly: {NOT_ENOUGH}");
    Ok(())
}
