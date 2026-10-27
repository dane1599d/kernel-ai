//! `test=desk-windows` (P3 task 6, ADR 0005): windows open, move and close.
//! `deskdemo` opens "Left" and "Right" through `kai-surface 1`; the case moves
//! the pointer to Left's title bar, presses, drags 200 px and lets go (Left
//! rises, takes focus and moves), then clicks Left's close control (`deskdemo`
//! destroys it and `desktop` closes it). Each step shows in `desktop`'s and
//! `deskdemo`'s log lines, which the host checks against `deskcore`'s
//! cascade: the first window's frame at 48,52, the second's at 80,84, each
//! 302x234 for 300x200 of content, on a 1280x800 screen.

use crate::display::claim;
use crate::pointer::{self, Event, Motion, Source};
use crate::{console, log, process, threads};

/// The plan lines this case adds.
pub const PLAN: &str = "desktop caps=display+input+serve pages=8192\n\
     deskdemo uses=desktop caps=call";
const WAIT_MS: u64 = 60_000;
/// Time for `desktop` to take each step in.
const STEP_MS: u64 = 400;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// A point on Left's title bar, clear of Right.
const LEFT_TITLE: (i32, i32) = (150, 60);
/// Left's close control once it has moved 200 px right: its frame is at
/// 248,52, the bar one px in, the control 2 px from the bar's right end.
const LEFT_CLOSE: (i32, i32) = (533, 69);

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

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    log!("desk-windows: timed out waiting for {what}");
    Err(what)
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop and deskdemo", || {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        shows.is_some_and(claim::shows) && !process::named("deskdemo").is_empty()
    })?;
    if !console::suspended() {
        return Err("desk-windows: the console still paints over the desktop");
    }
    // deskdemo's windows open with its first call; give it time to make it.
    threads::sleep_ms(2_000);
    let mut at = START;
    let mut go = |to: (i32, i32), buttons: u8| {
        report(to.0 - at.0, to.1 - at.1, buttons);
        at = to;
    };
    go(LEFT_TITLE, 0);
    go(LEFT_TITLE, 1);
    go((LEFT_TITLE.0 + 200, LEFT_TITLE.1), 1);
    go((LEFT_TITLE.0 + 200, LEFT_TITLE.1), 0);
    go(LEFT_CLOSE, 0);
    go(LEFT_CLOSE, 1);
    go(LEFT_CLOSE, 0);
    threads::sleep_ms(1_000);
    log!("desk-windows: dragged Left 200 px and pressed its close control");
    Ok(())
}

/// `test=desk-restart` (P3 task 6): `desktop` killed while it shows. The
/// console takes the screen back at once, the supervisor starts `desktop`
/// again, and `deskdemo` opens its windows again on the new one from the pool
/// it kept, without restarting.
pub fn restart() -> Result<(), &'static str> {
    let desktop = || process::named("desktop").first().map(|&(pid, _)| pid);
    let shows = |pid: u64| {
        process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
            .is_some_and(claim::shows)
    };
    wait("desktop and deskdemo", || {
        desktop().is_some_and(shows) && !process::named("deskdemo").is_empty()
    })?;
    threads::sleep_ms(2_000);
    let (Some(first), Some(&(demo, _))) = (desktop(), process::named("deskdemo").first()) else {
        return Err("desk-restart: desktop or deskdemo is gone");
    };
    if process::kill(first, kabi::ExitReason::Killed).is_err() {
        return Err("desk-restart: cannot kill desktop");
    }
    wait("the console to take the screen back", || {
        !console::suspended() && desktop() != Some(first)
    })?;
    wait("a new desktop to claim the screen", || {
        desktop().is_some_and(|pid| pid != first && shows(pid))
    })?;
    // deskdemo notices the new connection on its next poll.
    threads::sleep_ms(2_000);
    if process::named("deskdemo").first().map(|&(pid, _)| pid) != Some(demo) {
        return Err("desk-restart: deskdemo restarted instead of reconnecting");
    }
    log!("desk-restart: desktop was killed and came back, and deskdemo kept running");
    Ok(())
}
