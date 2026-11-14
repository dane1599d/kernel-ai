//! `test=ui-keys` (key input for `uikit` apps): the keyboard reaches an app's
//! widgets. Notes' window opens and takes the focus, its search field
//! focused (a window taking the keyboard focuses a text node, #872); the
//! harness types Tab and Enter, which `desktop` posts to the window's event
//! ring, comphost hands to `uikit`, and `uikit` routes to "New note", which
//! focuses the new note's editor; "hi" goes into it. The case reads the note
//! back.
//!
//! `busy`: the same keys while this case calls Notes every [`BUSY_MS`], more
//! often than comphost's 20 ms poll, from before the keys until after them.
//! comphost polls its windows on a deadline as well as when idle, so the keys
//! still arrive; every call is answered too.

use crate::display::claim;
use crate::{console, hpet, ipc, log, process, threads};
use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const NAME: &str = "notes-window";
/// The plan lines this case adds.
pub const PLAN: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1";
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// The harness's seven keys, a key each 150 ms after 200 ms, with room.
const KEYS_MS: u64 = 6_000;
/// How often `busy` calls Notes: well under comphost's 20 ms poll.
const BUSY_MS: u64 = 5;
/// The longest gap between two calls that still keeps comphost from ever
/// waiting out its 20 ms poll.
const MAX_GAP_MS: u64 = 15;
const COUNT: &str = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";

static BUSY: AtomicBool = AtomicBool::new(false);
static BUSY_DONE: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
/// Calls that came more than [`MAX_GAP_MS`] after the one before.
static LATE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    hpet::nanos() / 1_000_000
}

/// Whether `reply` is `count`'s answer: a `kai-reply` whose `result` line is
/// Notes' record.
fn counted(reply: &[u8]) -> bool {
    let text = String::from_utf8_lossy(reply);
    let mut lines = text.lines();
    lines
        .next()
        .is_some_and(|head| head.starts_with("kai-reply "))
        && lines.any(|line| line.starts_with("result {notes: "))
}

/// Calls Notes every [`BUSY_MS`] until told to stop.
fn hammer(_: usize) {
    let mut last = now_ms();
    while BUSY.load(Ordering::SeqCst) {
        // `count` answers `kai-reply 1`, then `result {notes: N, words: M}`.
        match ipc::call(NAME, COUNT.as_bytes(), CALL_MS) {
            Ok(reply) if counted(&reply) => {
                CALLS.fetch_add(1, Ordering::SeqCst);
            }
            reply => {
                if FAILED.fetch_add(1, Ordering::SeqCst) == 0 {
                    match reply {
                        Ok(reply) => log!(
                            "ui-keys: busy: count answered {:?}",
                            String::from_utf8_lossy(&reply)
                        ),
                        Err(error) => log!("ui-keys: busy: count failed: {error:?}"),
                    }
                }
            }
        }
        let now = now_ms();
        if now - last > MAX_GAP_MS {
            LATE.fetch_add(1, Ordering::SeqCst);
        }
        last = now;
        threads::sleep_ms(BUSY_MS);
    }
    BUSY_DONE.store(true, Ordering::SeqCst);
}

/// Stops [`hammer`] and checks it kept comphost busy throughout.
fn stop_hammer(started: u64) -> Result<(), &'static str> {
    BUSY.store(false, Ordering::SeqCst);
    while !BUSY_DONE.load(Ordering::SeqCst) {
        threads::sleep_ms(BUSY_MS);
    }
    let (calls, failed, late) = (
        CALLS.load(Ordering::SeqCst),
        FAILED.load(Ordering::SeqCst),
        LATE.load(Ordering::SeqCst),
    );
    let ms = now_ms() - started;
    log!(
        "ui-keys: busy: {calls} calls in {ms} ms, {failed} failed, {late} more than {MAX_GAP_MS} ms apart"
    );
    if failed > 0 {
        return Err("ui-keys: busy: a call to Notes failed");
    }
    // Most gaps must be short, or comphost waited out its poll anyway and the
    // case proves nothing.
    if calls == 0 || late * 10 > calls {
        return Err("ui-keys: busy: the calls did not keep comphost busy");
    }
    Ok(())
}

pub fn check(busy: bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && !process::named(NAME).is_empty() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        return Err("ui-keys: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    let started = now_ms();
    if busy {
        BUSY.store(true, Ordering::SeqCst);
        threads::spawn("ui-keys-busy", hammer, 0)?;
    }
    log!("ui-keys: ready for keys");
    threads::sleep_ms(KEYS_MS);
    if busy {
        stop_hammer(started)?;
    }
    let get = "kai-call 1\nfunc app:notes/actions@1.0.0#get\nargs {id: 1}\n";
    let reply = ipc::call(NAME, get.as_bytes(), CALL_MS)
        .map(|r| String::from_utf8_lossy(&r).into_owned())
        .map_err(|_| "ui-keys: notes-window did not reply")?;
    log!("ui-keys: note 1 is {reply}");
    if !reply.contains("some(\"hi\")") {
        return Err("ui-keys: the typed keys did not reach Notes' widgets");
    }
    if busy {
        log!("ui-keys: busy: the keys reached Notes while it was called every {BUSY_MS} ms");
    } else {
        log!(
            "ui-keys: Tab, Enter and typed text reached Notes' widgets through desktop and comphost"
        );
    }
    Ok(())
}
