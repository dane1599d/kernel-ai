//! `test=wait-reads` (the founder's test 2, item 7): the shell answers an app's
//! reads while the model thinks, and holds its changes until the line ends.
//! The test app `wait-window` (`apps/wait-fixture`, never shipped) is a window
//! with two buttons: a key on Read queues one `fs_list`, on Make one
//! `fs_create`, each as a person's effect through `desktop`. Its action `seen`
//! says how many keys it took and what came of each, in the order the answers
//! came, with the time from the key by its clock. The host sends the keys
//! when this case says it is ready for them. The mock model answers [`SLOW`]'s
//! `act` only after 8 s, and [`FAIL`]'s with an error after 6 s
//! (`xtask/src/mock_llm.rs`).
//!
//! 1. With no line running, a read is answered (the app's rows load).
//! 2. During [`SLOW`]: a read is answered in under [`READ_MS`], before the line
//!    ends. A change, and a read behind it, are not answered until the line
//!    has; then each is answered once, the change first.
//! 3. During [`FAIL`], a model call that fails partway: a read is answered in
//!    under [`READ_MS`], and the change held meanwhile runs once after it.
//!
//! The shell's log has each change once. There is no Stop for a line while it
//! waits on the model yet, so a model call that fails partway stands for one.

use crate::agent_desk_tests::{START, click};
use crate::display::claim;
use crate::{hpet, ipc, log, process, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// The shell with the file system and its route to `desktop`.
pub const SHELL: &str = kabi::plan::SHELL_DESK_FILES;
/// The test app, at boot, after `desktop`, its route.
pub const APPS: &str = "wait-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1";

const CASE: &str = "wait-reads";
/// `xtask/src/mock_llm.rs`'s `SLOW_LINE` and `FAIL_LINE`.
const SLOW: &str = "take a while over the cpu count";
const FAIL: &str = "stop partway through the cpu count";
/// The longest an app's read may wait while the model thinks.
const READ_MS: u64 = 1_000;
/// How long after a line is sent the shell is surely waiting on the model.
const IN_CALL_MS: u64 = 700;
/// The app's window: the first frame, at 48,52, and its title bar 8 px down.
const TITLE: (i32, i32) = (150, 60);
/// Time for comphost to start the app and paint its window under TCG.
const OPEN_MS: u64 = 20_000;
/// How often `seen` is asked.
const SEEN_MS: u64 = 20;
/// How long the shell may take, once it has answered a line, to have the
/// line's reply reach this case's thread: an answer seen in that time came
/// at the line's end, not during it. A held request answered too early would
/// come seconds before.
const END_MS: u64 = 500;
/// How long the host's keys and their answers may take with no line running,
/// and a held change once its line has ended.
const KEYS_MS: u64 = 30_000;
const AFTER_MS: u64 = 15_000;
const LINE_MS: u64 = 60_000;
const CALL_MS: u64 = 10_000;

/// The lines, by index, and the one running on its own thread: whether it has
/// answered, and whether the answer was `ok`.
const LINES: [&str; 2] = [SLOW, FAIL];
static LINE_DONE: AtomicBool = AtomicBool::new(true);
static LINE_OK: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    hpet::nanos() / 1_000_000
}

fn line_running() -> bool {
    !LINE_DONE.load(Ordering::SeqCst)
}

/// Sends `LINES[at]` to the shell, as the terminal does, and says when it
/// answered.
fn run_line(at: usize) {
    let line = LINES[at];
    let start = now_ms();
    let text = match ipc::call("shell", line.as_bytes(), LINE_MS) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::from("no answer"),
    };
    // Said first: the log line takes a while, and the shell runs what it held
    // as soon as it has answered.
    LINE_OK.store(text.starts_with("ok"), Ordering::SeqCst);
    LINE_DONE.store(true, Ordering::SeqCst);
    log!(
        "{CASE}: {line:?} answered {:?} after {} ms",
        text.replace('\n', "; "),
        now_ms() - start
    );
}

/// Starts `LINES[at]` on a thread of its own, and waits until the shell is
/// surely waiting on the model for it.
fn start_line(at: usize) -> Result<(), &'static str> {
    LINE_DONE.store(false, Ordering::SeqCst);
    LINE_OK.store(false, Ordering::SeqCst);
    threads::spawn("wait-line", run_line, at)?;
    threads::sleep_ms(IN_CALL_MS);
    if !line_running() {
        log!("{CASE}: {:?} answered before the model could", LINES[at]);
        return Err("wait-reads: a line ended before its model call");
    }
    Ok(())
}

/// One answer the app had: its effect's ticket, whether it was a change,
/// whether it was ok, and how long it took from the key.
#[derive(Clone, Copy)]
struct Answer {
    ticket: u32,
    make: bool,
    ok: bool,
    ms: u64,
}

/// What the app says it took: its keys, and its answers in the order they came.
fn seen() -> Result<(u32, Vec<Answer>), &'static str> {
    let request = "kai-call 1\nfunc app:wait/actions@1.0.0#seen\nargs {}\n";
    let reply = ipc::call("wait-window", request.as_bytes(), CALL_MS)
        .map_err(|_| "wait-reads: the app did not answer")?;
    let text = String::from_utf8_lossy(&reply).into_owned();
    // `ok("...")`: the action's string, with WAVE's escape for newlines.
    let inner = text
        .split_once('"')
        .and_then(|(_, rest)| rest.rsplit_once('"'))
        .map(|(inner, _)| inner.replace("\\n", "\n"))
        .ok_or("wait-reads: the app's seen was not a string")?;
    let mut lines = inner.lines();
    let pressed = lines
        .next()
        .and_then(|first| first.strip_prefix("pressed "))
        .and_then(|n| n.parse().ok())
        .ok_or("wait-reads: the app did not say its keys")?;
    let answers = lines
        .map(|line| {
            let words: Vec<&str> = line.split(' ').collect();
            match words.as_slice() {
                [ticket, kind, word, ms] => Some(Answer {
                    ticket: ticket.parse().ok()?,
                    make: *kind == "make",
                    ok: *word == "ok",
                    ms: ms.parse().ok()?,
                }),
                _ => None,
            }
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("wait-reads: the app's answers did not parse")?;
    Ok((pressed, answers))
}

/// Says the case is ready for the host's next keys.
fn keys() {
    log!("{CASE}: ready for keys");
}

/// Waits until the app has taken `presses` keys, the line still running.
fn pressed_while_running(presses: u32) -> Result<(), &'static str> {
    loop {
        let (pressed, _) = seen()?;
        if pressed >= presses {
            if !line_running() {
                log!("{CASE}: the keys came only after the line had ended");
                return Err("wait-reads: the keys did not come while the model thought");
            }
            return Ok(());
        }
        if !line_running() {
            return Err("wait-reads: the line ended before the keys came");
        }
        threads::sleep_ms(SEEN_MS);
    }
}

/// Waits until the app has `count` answers, within `limit_ms`.
fn answers(count: usize, limit_ms: u64) -> Result<Vec<Answer>, &'static str> {
    let deadline = now_ms() + limit_ms;
    loop {
        let (_, answers) = seen()?;
        if answers.len() >= count {
            return Ok(answers);
        }
        if now_ms() > deadline {
            log!(
                "{CASE}: {} answers of {count} after {limit_ms} ms",
                answers.len()
            );
            return Err("wait-reads: an effect was never answered");
        }
        threads::sleep_ms(SEEN_MS);
    }
}

/// The key just sent was a read: answered ok in under [`READ_MS`] from the
/// key, by the app's clock, while the line still ran (`seen` is asked before
/// the line's state, so an answer seen with the line running came first).
fn read_while_thinking(count: usize) -> Result<u64, &'static str> {
    let deadline = now_ms() + KEYS_MS;
    let read = loop {
        let (_, answers) = seen()?;
        let running = line_running();
        if answers.len() >= count {
            if !running {
                return Err("wait-reads: a read was answered only after the line");
            }
            break answers[count - 1];
        }
        if !running || now_ms() > deadline {
            return Err("wait-reads: a read was not answered while the model thought");
        }
        threads::sleep_ms(SEEN_MS);
    };
    if read.make || !read.ok || read.ms >= READ_MS {
        log!(
            "{CASE}: the read was ok {} after {} ms from its key",
            read.ok,
            read.ms
        );
        return Err("wait-reads: a read during the model's call was slow or refused");
    }
    Ok(read.ms)
}

/// Until the line ends, the app has no more than `count` answers. One seen
/// with the line still running passes only if the line's reply comes within
/// [`END_MS`]: the shell had answered the line already.
fn held_until_the_line_ends(count: usize) -> Result<(), &'static str> {
    while line_running() {
        let (_, answers) = seen()?;
        if answers.len() > count && line_running() {
            let seen_at = now_ms();
            while line_running() && now_ms() - seen_at < END_MS {
                threads::sleep_ms(SEEN_MS);
            }
            if line_running() {
                log!(
                    "{CASE}: effect {} was answered while the line still ran",
                    answers[count].ticket
                );
                return Err("wait-reads: a held effect ran before the line ended");
            }
        }
        threads::sleep_ms(SEEN_MS);
    }
    Ok(())
}

/// The shell's log has `ticket`'s effect exactly once.
fn logged_once(ticket: u32) -> Result<(), &'static str> {
    let reply = ipc::call("shell", b"log", CALL_MS)
        .map_err(|_| "wait-reads: the shell did not answer log")?;
    let history = String::from_utf8_lossy(&reply);
    let entry = format!("wait-window: effect {ticket}");
    let count = history
        .lines()
        .filter(|line| {
            line.split_once(&entry)
                .is_some_and(|(_, rest)| !rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .count();
    if count != 1 {
        log!("{CASE}: the log has {entry:?} {count} times");
        return Err("wait-reads: a held change did not run exactly once");
    }
    Ok(())
}

/// The app's window, given the keys, and a read with no line running.
fn first_read() -> Result<(), &'static str> {
    let deadline = now_ms() + OPEN_MS * 3;
    while !claim::showing() || process::named("wait-window").is_empty() {
        if now_ms() > deadline {
            return Err("wait-reads: desktop or the app never started");
        }
        threads::sleep_ms(100);
    }
    threads::sleep_ms(OPEN_MS);
    let mut at = START;
    click(&mut at, TITLE);
    threads::sleep_ms(1_000);
    // `tab ret`: Read.
    keys();
    let first = answers(1, KEYS_MS)?;
    if first[0].make || !first[0].ok {
        return Err("wait-reads: the first read was refused");
    }
    log!(
        "{CASE}: with no line running, a read was answered in {} ms",
        first[0].ms
    );
    Ok(())
}

fn slow_line() -> Result<(), &'static str> {
    start_line(0)?;
    // `ret`: Read.
    keys();
    let took = read_while_thinking(2)?;
    log!("{CASE}: while the model thought, the app's read was answered in {took} ms");
    // `tab ret tab ret`: Make, then Read behind it.
    keys();
    pressed_while_running(4)?;
    held_until_the_line_ends(2)?;
    if !LINE_OK.load(Ordering::SeqCst) {
        return Err("wait-reads: the slow line did not answer ok");
    }
    let after = answers(4, AFTER_MS)?;
    let (made, behind) = (after[2], after[3]);
    if !made.make || !made.ok || behind.make || !behind.ok {
        return Err("wait-reads: the held change failed or came after the read behind it");
    }
    logged_once(made.ticket)?;
    log!(
        "{CASE}: the app's change and the read behind it waited for the line, then ran once each, in order"
    );
    Ok(())
}

fn failing_line() -> Result<(), &'static str> {
    start_line(1)?;
    // `ret`: Read.
    keys();
    let took = read_while_thinking(5)?;
    // `tab ret`: Make.
    keys();
    pressed_while_running(6)?;
    held_until_the_line_ends(5)?;
    let after = answers(6, AFTER_MS)?;
    let made = after[5];
    if !made.make || !made.ok {
        return Err("wait-reads: the change held during the failed call did not run");
    }
    logged_once(made.ticket)?;
    log!(
        "{CASE}: a model call that failed partway ended its line; the read during it took {took} ms and the change held during it ran once after"
    );
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    first_read()?;
    slow_line()?;
    failing_line()?;
    log!(
        "{CASE}: the shell answered the app's reads while the model thought, and held its changes until the line ended"
    );
    Ok(())
}
