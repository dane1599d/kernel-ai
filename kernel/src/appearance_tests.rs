//! `test=desk-appearance` (P4 task 8): a saved appearance holds, live and
//! across a `desktop` restart. The v0.1 system (`kabi::plan::system`) on a
//! `kai-data` disk; the case reads the screen's own pixels, a point of the
//! empty desk, against each shipped theme's desk colour:
//! - at first the desk is dark;
//! - "set the theme to light", typed as the user would, runs `appearance_set`
//!   (the mock model's call); the shell saves it and hands it to `desktop`,
//!   and the desk turns light without a restart;
//! - `desktop` is killed; the supervisor starts it again from its plan line
//!   (dark), and the shell, seeing a new `desktop`, gives it the saved
//!   appearance: the desk is light again within seconds;
//! - "set the text size to 150" and "open notes" (the person's own line): the
//!   app's `comphost`, greeted with the saved appearance, logs that it lays out
//!   at the saved text size (`comphost: laying out at text size 150%`).

use crate::{desk_tests, display, ipc, log, process, terminal, threads};
use core::sync::atomic::{AtomicUsize, Ordering};
use kabi::ExitReason;

/// A point of the empty desk at 1280x800, clear of every window and of the
/// pointer at the middle.
const DESK_POINT: (usize, usize) = {
    let (x, y) = crate::ui_tests::DESK.desk_point();
    (x as usize, y as usize)
};
/// The lines typed, in order: the theme, the text size, then an app.
const LINES: [&str; 3] = [
    "set the theme to light",
    "set the text size to 150",
    "open notes",
];
/// How long Notes may take to start and greet `desktop`, and its first pump
/// to lay out and log (under TCG a component's first frame takes seconds).
const NOTES_MS: u64 = 60_000;
const LAYOUT_MS: u64 = 10_000;
/// How long the light desk may take: the shell looks for a new `desktop`
/// every 2 s, and a restart paints its first frames before that.
const LIGHT_MS: u64 = 30_000;

/// How many of [`LINES`] the shell has answered.
static DONE: AtomicUsize = AtomicUsize::new(0);

fn liner(line: usize) {
    terminal::shell_line_answered(LINES[line]);
    DONE.fetch_add(1, Ordering::Release);
}

/// Types [`LINES`]`[line]` as the user would and waits for the shell's answer.
fn type_line(line: usize) -> Result<(), &'static str> {
    threads::spawn("liner", liner, line)?;
    desk_tests::wait("the shell's answer", || DONE.load(Ordering::Acquire) > line)
}

/// The screen's pixel at [`DESK_POINT`], as `0xRRGGBB`.
fn desk_pixel() -> Option<u32> {
    let surface = display::surface()?;
    let (x, y) = DESK_POINT;
    if x >= surface.width || y >= surface.height {
        return None;
    }
    // SAFETY: the surface is `stride * height` pixels from `base`, and the
    // point is inside it (checked above); a read races only `desktop`'s writes.
    let pixel = unsafe {
        surface
            .base
            .as_ptr()
            .add(y * surface.stride + x)
            .read_volatile()
    };
    Some(pixel & 0x00FF_FFFF)
}

/// Each shipped theme's desk colour, by name.
fn desk_of(name: &str) -> Option<u32> {
    look::theme::shipped()
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.palette().desk & 0x00FF_FFFF)
}

fn desktop() -> Option<u64> {
    process::named("desktop").first().map(|&(pid, _)| pid)
}

fn desk_is(what: &'static str, colour: u32) -> Result<(), &'static str> {
    for _ in 0..LIGHT_MS / 100 {
        if desk_pixel() == Some(colour) {
            return Ok(());
        }
        threads::sleep_ms(100);
    }
    log!(
        "desk-appearance: waited for {what}; the desk is {:#08x}",
        desk_pixel().unwrap_or(0)
    );
    Err(what)
}

pub fn check() -> Result<(), &'static str> {
    let (dark, light) = (
        desk_of("dark").ok_or("desk-appearance: no dark theme")?,
        desk_of("light").ok_or("desk-appearance: no light theme")?,
    );
    desk_tests::wait("desktop to draw", || {
        desk_tests::desktop_shows() && display::claim::flushes() > 0
    })?;
    desk_tests::wait("the shell", || ipc::server_of("shell").is_some())?;
    desk_is("the dark desk at first", dark)?;
    type_line(0)?;
    desk_is("the light desk after the change", light)?;
    log!("desk-appearance: the saved light theme painted the desk at once");
    let first = desktop().ok_or("desk-appearance: no desktop")?;
    if process::kill(first, ExitReason::Killed).is_err() {
        return Err("desk-appearance: cannot kill desktop");
    }
    desk_tests::wait("a new desktop to claim the screen", || {
        desktop().is_some_and(|pid| pid != first) && desk_tests::desktop_shows()
    })?;
    desk_is("the light desk after desktop restarted", light)?;
    log!("desk-appearance: the restarted desktop painted the saved light theme");
    // The saved text size reaches an app's windows: Notes starts, greets
    // `desktop`, hears the whole appearance and lays out in it.
    type_line(1)?;
    type_line(2)?;
    for _ in 0..NOTES_MS / 100 {
        if !process::named("notes-window").is_empty() {
            break;
        }
        threads::sleep_ms(100);
    }
    if process::named("notes-window").is_empty() {
        return Err("desk-appearance: notes-window did not start");
    }
    threads::sleep_ms(LAYOUT_MS);
    log!("desk-appearance: Notes started at the saved text size");
    Ok(())
}
