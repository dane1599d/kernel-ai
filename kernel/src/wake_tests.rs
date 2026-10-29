//! `test=event-wake` (P4, the event-box wake; lane K's checklist):
//! - the box's reader is the process EVENT_READ admits (the shell);
//! - a post just before the wait still wakes it, and never in place of a
//!   line: before each of ten terminal lines the kernel posts `work`, and every
//!   line comes back as its own answer, the line first;
//! - a post with no line behind it leaves the shell serving, not spinning;
//! - once the shell exits, the box has no reader: the restarted shell, a new
//!   pid, inherits nothing until it reads.

use crate::shell_tests::Expect::Exact;
use crate::shell_tests::expect_lines;
use crate::{event_box, ipc, log, process, threads};
use kabi::event::Event;

const LINES: usize = 10;

fn post_work() -> Result<(), &'static str> {
    let work = Event::parse("work").ok_or("event-wake: `work` does not parse")?;
    event_box::post(work);
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    for _ in 0..600 {
        if ipc::server_of("shell").is_some() {
            break;
        }
        threads::sleep_ms(100);
    }
    // The shell reads the box once it serves, so it is the box's reader.
    expect_lines(&[("1 + 1", Exact("ok 2"))])?;
    for _ in 0..LINES {
        post_work()?;
        expect_lines(&[("40 + 2", Exact("ok 42"))])?;
    }
    // A wake with no line behind it: the shell takes the box and waits again.
    post_work()?;
    threads::sleep_ms(200);
    expect_lines(&[("6 * 7", Exact("ok 42"))])?;
    log!("event-wake: {LINES} terminal lines, each after a post, all came back as lines");
    let shell = ipc::server_of("shell").ok_or("event-wake: the shell is gone")?;
    if event_box::reader() != shell {
        return Err("event-wake: the shell is not the box's reader");
    }
    process::kill(shell, kabi::ExitReason::Killed)
        .map_err(|_| "event-wake: cannot stop the shell")?;
    for _ in 0..100 {
        if ipc::server_of("shell").is_some_and(|pid| pid != shell) {
            break;
        }
        threads::sleep_ms(50);
    }
    if event_box::reader() == shell {
        return Err("event-wake: the box kept the ended shell as its reader");
    }
    // The new shell reads the box, and is woken from then on.
    expect_lines(&[("1 + 2", Exact("ok 3"))])?;
    post_work()?;
    expect_lines(&[("2 + 2", Exact("ok 4"))])?;
    log!("event-wake: the ended shell's wake went with it; the new shell reads the box");
    Ok(())
}
