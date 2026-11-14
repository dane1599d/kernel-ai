//! Whether the model is warm, for the panel (founder test 2). After a rebuild
//! that changes the prompt, llm fills its two prompt slots again before a
//! request runs at full speed, about 17 s. Until it says both are warm, the
//! panel says "Kai is getting ready" instead of a bare "thinking". The rules
//! (what cools llm, and what a "no" means once it has said "yes") are
//! `deskcore::warmth`'s; this asks llm and tells `desktop`.
//!
//! llm answers `warm` from its serve loop, and while it fills its slots it
//! reads a request only between compute slices of about a second. So the
//! shell asks two ways:
//! - right before it sends the panel's line ([`Warmth::before_request`]),
//!   waiting up to [`ASK_MS`]: the person waits on llm then anyway. It asks
//!   again once the answer is in ([`Warmth::after_request`]);
//! - between lines, every few seconds, with a short wait ([`PEEK_MS`]) that
//!   never holds up the shell's loop: an idle llm answers at once, and a busy
//!   one changes nothing.
//!
//! What `desktop` is told is `deskcore::panel_wire`'s `panel ready`: when it
//! changes, and again to a `desktop` that restarted.

use super::{ask_desktop, desktop, desktop_pid, llm};
use core::sync::atomic::{AtomicBool, Ordering};
use deskcore::panel_wire;
use deskcore::warmth::{Answer, Readiness};
use user::{println, sys};

/// The wait between lines: short, so the loop never waits on a busy llm.
const PEEK_MS: u64 = 20;
/// The wait before a panel line: longer than one of llm's warm-up slices.
const ASK_MS: u64 = 1_500;

/// Set when the shell made llm fill its slots again.
static COOLED: AtomicBool = AtomicBool::new(false);
/// The language llm keeps warm, as the shell last told it: Rhai at start.
static LANG_C: AtomicBool = AtomicBool::new(false);

/// The shell made llm fill its slots again (`new`).
pub(crate) fn cooled() {
    COOLED.store(true, Ordering::Relaxed);
}

/// The shell told llm which language to keep warm (`warm-lang`): a change
/// cools it, as llm does (`cold |= lang != warm_lang`).
pub(crate) fn lang(c: bool) {
    if LANG_C.swap(c, Ordering::Relaxed) != c {
        cooled();
    }
}

/// What the shell believes, and what it last told which `desktop`.
#[derive(Default)]
pub(crate) struct Warmth {
    readiness: Readiness,
    told: Option<(bool, u64)>,
}

impl Warmth {
    /// Between lines, when it is time: a short ask.
    pub(crate) fn peek(&mut self) {
        if !self.readiness.due(sys::time_ns()) {
            return;
        }
        if self.look() {
            self.ask(PEEK_MS, false);
        }
        self.tell();
    }

    /// Right before the panel's line goes to llm.
    pub(crate) fn before_request(&mut self) {
        if self.look() {
            self.ask(ASK_MS, true);
        }
        self.tell();
    }

    /// The panel's line has its answer: llm is free, so the next pass asks.
    pub(crate) fn after_request(&mut self) {
        self.readiness.ask_soon();
    }

    /// Whether llm runs, after noting what cooled it since: a `new`, a
    /// language switch, or a restart.
    fn look(&mut self) -> bool {
        let pid = llm_pid();
        let cooled = COOLED.swap(false, Ordering::Relaxed);
        self.readiness.look(pid, cooled);
        pid.is_some()
    }

    /// llm's answer to `warm` within `wait_ms`; `waits` before a panel line.
    fn ask(&mut self, wait_ms: u64, waits: bool) {
        let mut reply = [0u8; 16];
        let answer = match sys::call_route(llm(), b"warm", &mut reply, wait_ms) {
            Ok(len) => match reply.get(..len) {
                Some(b"ok yes") => Answer::Yes,
                Some(b"ok no") => Answer::No,
                _ => return,
            },
            Err(kabi::Error::TimedOut) => Answer::Late,
            Err(_) => return,
        };
        self.readiness.answer(answer, waits);
    }

    /// What the panel should say, to `desktop`, unless that one was told it.
    fn tell(&mut self) {
        let (Some(ready), Some(to)) = (self.readiness.want(), desktop_pid()) else {
            return;
        };
        if self.told == Some((ready, to)) {
            return;
        }
        let Some(route) = desktop() else {
            return;
        };
        match ask_desktop(route, &panel_wire::encode_ready(ready)) {
            Ok(answer) if answer == "ok" => self.told = Some((ready, to)),
            Ok(answer) => println!("the panel was not told the model's warmth: {answer}"),
            Err(why) => println!("the panel was not told the model's warmth: {why}"),
        }
    }
}

/// The running `llm`'s pid, from the kernel's METRICS.
fn llm_pid() -> Option<u64> {
    let metrics = sys::metrics().ok()?;
    metrics
        .processes
        .iter()
        .take(metrics.process_count as usize)
        .find(|p| p.name() == "llm")
        .map(|p| p.pid)
}
