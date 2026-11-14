//! Whether the model is warm, for the panel (founder test 2). After a rebuild
//! that changes the prompt, llm fills its two prompt slots again before a
//! request runs at full speed, about 17 s. Until it says both are warm, the
//! panel says "Kai is getting ready" instead of a bare "thinking".
//!
//! llm answers `warm` from its serve loop, and while it fills its slots it
//! reads a request only between compute slices of about a second. So the
//! shell asks two ways:
//! - right before it sends the panel's line ([`Warmth::before_request`]),
//!   waiting up to [`ASK_MS`]: the person waits on llm then anyway. An ask
//!   that times out before llm has said "yes" counts as not warm, so the panel
//!   says "Kai is getting ready" for the request that waits on the warm-up. It
//!   asks again once the answer is in ([`Warmth::after_request`]);
//! - between lines, every few seconds, with a short wait ([`PEEK_MS`]) that
//!   never holds up the shell's loop: an idle llm answers at once, and a busy
//!   one changes nothing.
//!
//! A `new` and a switch between `c>` and Rhai make llm fill its slots again,
//! as does a restart of llm; each makes llm unconfirmed until it says "yes".
//! The brief re-compaction after a remembered turn does not. What `desktop`
//! is told is `deskcore::panel_wire`'s `panel ready`: when it changes, and
//! again to a `desktop` that restarted.

use super::{ask_desktop, desktop, desktop_pid, llm};
use core::sync::atomic::{AtomicBool, Ordering};
use deskcore::panel_wire;
use user::{println, sys};

/// How often llm is asked between lines while it is not confirmed warm.
const COLD_ASK_NS: u64 = 3_000_000_000;
/// And once it is: a restart or a missed re-warm shows within this.
const WARM_ASK_NS: u64 = 30_000_000_000;
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

/// What the shell believes and last told, and when it last asked.
#[derive(Default)]
pub(crate) struct Warmth {
    /// llm said "yes" since the last thing that cooled it.
    confirmed: bool,
    /// What `desktop` was last told, and which `desktop`.
    told: Option<(bool, u64)>,
    asked_ns: u64,
    llm_pid: Option<u64>,
}

impl Warmth {
    /// Between lines, when it is time: a short ask.
    pub(crate) fn peek(&mut self) {
        let now = sys::time_ns();
        let gap = if self.confirmed {
            WARM_ASK_NS
        } else {
            COLD_ASK_NS
        };
        if self.asked_ns != 0 && now.saturating_sub(self.asked_ns) < gap {
            return;
        }
        self.asked_ns = now.max(1);
        if self.look() {
            match ask(PEEK_MS) {
                Ok(Some(ready)) => self.tell(ready),
                // Busy, or no answer: nothing changes between lines.
                Ok(None) | Err(_) => self.retell(),
            }
        }
    }

    /// Right before the panel's line goes to llm.
    pub(crate) fn before_request(&mut self) {
        if !self.look() {
            return;
        }
        match ask(ASK_MS) {
            Ok(Some(ready)) => self.tell(ready),
            // Busy before it has said "yes": it is still filling its slots.
            Ok(None) if !self.confirmed => self.tell(false),
            Ok(None) | Err(_) => {}
        }
    }

    /// The panel's line has its answer: llm is free, so the next pass asks.
    pub(crate) fn after_request(&mut self) {
        self.asked_ns = 0;
    }

    /// Whether llm runs, after taking note of what cooled it since: a `new`,
    /// a language switch, or llm restarted. With no llm, a panel told "getting
    /// ready" is told otherwise, and nothing is asked.
    fn look(&mut self) -> bool {
        let pid = llm_pid();
        if COOLED.swap(false, Ordering::Relaxed) || (self.llm_pid.is_some() && pid != self.llm_pid)
        {
            self.confirmed = false;
        }
        self.llm_pid = pid;
        if pid.is_none() && matches!(self.told, Some((false, _))) {
            self.tell(true);
        }
        pid.is_some()
    }

    /// `ready` to `desktop`, when it differs from what it was told.
    fn tell(&mut self, ready: bool) {
        self.confirmed |= ready;
        let Some(to) = desktop_pid() else {
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

    /// What was told, again, to a `desktop` that restarted since.
    fn retell(&mut self) {
        if let Some((ready, to)) = self.told
            && desktop_pid().is_some_and(|now| now != to)
        {
            self.tell(ready);
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

/// llm's answer to `warm` within `wait_ms`: whether both prompt slots hold
/// their prompts, `None` when it did not answer in time, or why not.
fn ask(wait_ms: u64) -> Result<Option<bool>, kabi::Error> {
    let mut reply = [0u8; 16];
    let len = match sys::call_route(llm(), b"warm", &mut reply, wait_ms) {
        Ok(len) => len,
        Err(kabi::Error::TimedOut) => return Ok(None),
        Err(error) => return Err(error),
    };
    match reply.get(..len) {
        Some(b"ok yes") => Ok(Some(true)),
        Some(b"ok no") => Ok(Some(false)),
        _ => Err(kabi::Error::Invalid),
    }
}
