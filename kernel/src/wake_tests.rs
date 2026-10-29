//! `test=event-wake` (P4, the event-box wake): a post to the event box wakes
//! the shell's `svc_recv` with `Woken`, never in place of a line. Before each
//! of ten terminal lines the kernel posts `work`; every line must come back as
//! its own answer, and the shell must still be serving after a post with no
//! line behind it.

use crate::shell_tests::Expect::Exact;
use crate::shell_tests::expect_lines;
use crate::{event_box, ipc, log, threads};
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
    Ok(())
}
