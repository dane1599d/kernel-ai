//! The model manager's `kai:models` reads (P4 task 10): when the shown version
//! has a read waiting and the last list is old enough, comphost asks `updated`
//! for `models` over its route and gives the app the answer through
//! `listing.on-listing`, a call of its own like a window's input.
//!
//! As with `kai:system` (`system.rs`), a read is never held here. It waits in
//! the instance's own queue until it can be answered in the same step, so a
//! reloaded, resumed or swapped instance starts with none. A read waiting is
//! use, so the app does not nap until it has its list.
//!
//! Only the process the system image names [`MODELS_APP`] is granted
//! `kai:models` (its plan line, never its package, and only for the image's
//! own versions), and only it has a route to `updated`, which answers that
//! process `models` and nothing else (`modelchan::asker`). The window changes
//! nothing: a switch or an import is Kai's row.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::Error;
use kabi::handle::Handle;
use livecore::host::{Env, Host};
// The one request the window's reads become.
use modelchan::asker::LIST;
use user::{println, sys};
use wasmhost::LiveInstance;

/// The one app the image grants `kai:models`: the plan line's name for it,
/// which is its component's, and the name `updated` knows the window by.
pub use modelchan::asker::WINDOW as MODELS_APP;

/// Least time between two lists, in milliseconds. The window asks when it opens
/// and when the person refreshes it; this keeps a held key from asking again
/// and again.
const PERIOD_MS: u64 = 1_000;
/// How long `updated` may take. It checks the signed role, lists the store and
/// asks llm for its `health` (at most 3 s while the model is busy), one request
/// at a time; the window waits for the list only when it asks.
const LIST_MS: u64 = 5_000;
/// How many times in a row a read waits on `updated` not serving yet before it
/// is told so: `updated` starts at boot, and the window may ask before it
/// serves. The first try waits up to [`LIST_MS`]; each one after it up to
/// [`NOT_YET_MS`], with as long again before the next, so the window answers
/// keys between tries (lane C's review of #568). About 30 s in all.
const STARTING_TRIES: u32 = 30;
/// A later try's wait for `updated` to serve, and the pause after it.
const NOT_YET_MS: u64 = 500;

/// Why there is no list.
enum NoList {
    /// `updated` is not serving yet: worth asking again.
    NotYet(String),
    Why(String),
}

pub struct Lister {
    /// The route to `updated`: the second, so the plan line's `uses=` must be
    /// `desktop+updated`, which the image's plan line for [`MODELS_APP`] pins
    /// with a test. With no second route, each read is told why.
    updated: Option<Handle>,
    /// The earliest time the next list may be asked for, in milliseconds.
    next_ms: u64,
    reply: Vec<u8>,
    /// Whether the log has said that the first list reached the app.
    said: bool,
    /// The tries in a row that found `updated` not serving yet.
    tries: u32,
}

impl Lister {
    pub fn new() -> Self {
        Self {
            updated: user::route(1),
            next_ms: 0,
            reply: alloc::vec![0u8; kabi::MAX_MESSAGE],
            said: false,
            tries: 0,
        }
    }

    /// Whether the shown version has a read waiting. A standby's reads are
    /// dropped, so a version rehearsed beside the shown one asks for nothing.
    pub fn waiting<E: Env<Instance = LiveInstance>>(&self, host: &mut Host<E>) -> bool {
        let mut waiting = false;
        host.each_instance(|instance, _, shown| {
            if let LiveInstance::V2(core) = instance {
                if shown {
                    waiting = core.models_waiting();
                } else {
                    drop(core.take_models());
                }
            }
        });
        waiting
    }

    /// How long until a waiting read may be answered, for the host's wait; 0 if
    /// it may be now, `None` when none waits.
    pub fn wait_ms<E: Env<Instance = LiveInstance>>(
        &self,
        host: &mut Host<E>,
        now_ms: u64,
    ) -> Option<u64> {
        self.waiting(host)
            .then(|| self.next_ms.saturating_sub(now_ms))
    }

    /// Answers the waiting reads if one is due; whether it did. While `updated`
    /// is not serving yet the reads stay waiting, for [`STARTING_TRIES`] tries.
    pub fn pump<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
        now_ms: u64,
    ) -> bool {
        if self.wait_ms(host, now_ms) != Some(0) {
            return false;
        }
        self.next_ms = now_ms.saturating_add(PERIOD_MS);
        let list = match self.list() {
            Err(NoList::NotYet(_)) if self.tries < STARTING_TRIES => {
                self.tries += 1;
                // From when this try ended, which may be a full wait later.
                let ended_ms = sys::time_ns() / 1_000_000;
                self.next_ms = ended_ms.saturating_add(NOT_YET_MS);
                return false;
            }
            Err(NoList::NotYet(why) | NoList::Why(why)) => Err(why),
            Ok(list) => Ok(list),
        };
        self.tries = 0;
        let bytes = list.as_ref().map(String::len);
        let answered = crate::ui::serve_shown(host, |core| {
            for ticket in core.take_models() {
                core.models_listing(ticket, list.clone())?;
            }
            Ok(())
        });
        match (answered, bytes) {
            (None, _) => println!("comphost: the model list did not reach the app"),
            // Said once, so a log shows that the window was given the list.
            (Some(_), Ok(len)) if !self.said => {
                self.said = true;
                println!("comphost: the first model list reached the window ({len} bytes)");
            }
            (Some(_), Err(why)) => println!("comphost: no model list: {why}"),
            _ => {}
        }
        true
    }

    /// `updated`'s list, or why there is none.
    fn list(&mut self) -> Result<String, NoList> {
        let route = self.updated.ok_or_else(|| {
            NoList::Why(String::from(
                "this window has no route to the update service",
            ))
        })?;
        let starting = self.tries > 0;
        let wait_ms = if starting { NOT_YET_MS } else { LIST_MS };
        let len =
            sys::call_route(route, LIST.as_bytes(), &mut self.reply, wait_ms).map_err(|error| {
                let why = format!("the update service did not answer ({error:?})");
                match error {
                    // The kernel waited the call's time for it to serve.
                    Error::Unavailable => NoList::NotYet(why),
                    // A short try may reach it as it starts serving.
                    Error::TimedOut if starting => NoList::NotYet(why),
                    _ => NoList::Why(why),
                }
            })?;
        let text = core::str::from_utf8(&self.reply[..len.min(self.reply.len())])
            .map_err(|_| NoList::Why(String::from("the update service's list is not text")))?;
        match text.strip_prefix("err ") {
            Some(why) => Err(NoList::Why(String::from(why))),
            None => Ok(String::from(text)),
        }
    }
}
