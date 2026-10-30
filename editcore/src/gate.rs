//! What comphost checks before it sends an app's message to the edit service
//! (P4 task 11). The `edit` grant includes starting host builds of the app's own
//! typed text, which the build service takes as the founder's, so a build is
//! sent only when a person asked for it:
//!
//! - the message was queued while a real key or click was being handled, never
//!   from `init`, an answer, a timer or the agent's `ui_act`; and
//! - not within [`BUILD_GAP_MS`] of the last build this gate let through, so a
//!   loop of presses cannot keep the host compiling.
//!
//! The rest of what an app may send is the kernel's check
//! ([`crate::ipc::for_app`]). Pure logic, tested on the host.

use crate::ipc::{Ipc, decode_ipc};

/// The least time between two builds an app starts.
pub const BUILD_GAP_MS: u64 = 10_000;

/// Why a build was not sent, in the words the app shows.
pub const NOT_BY_A_PERSON: &str = "err a build starts only from a key or click";
pub const TOO_SOON: &str = "err a build ran a moment ago; try again in a few seconds";

/// Whether `message` is the command that starts a build.
pub fn starts_build(message: &[u8]) -> bool {
    matches!(decode_ipc(message), Ok(Ipc::Cmd(line)) if line.split(' ').next() == Some("build"))
}

/// The state the rules need: when the last build went through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gate {
    last_build_ms: Option<u64>,
}

impl Gate {
    pub const fn new() -> Self {
        Self {
            last_build_ms: None,
        }
    }

    /// Lets `message` through, or says why not (an `err ...` line for the app).
    /// `user` is whether a person's key or click was being handled when the app
    /// sent it; `now_ms` is any clock that does not run backwards.
    pub fn check(&mut self, message: &[u8], user: bool, now_ms: u64) -> Result<(), &'static str> {
        if !starts_build(message) {
            return Ok(());
        }
        if !user {
            return Err(NOT_BY_A_PERSON);
        }
        let waited = self.last_build_ms.map(|at| now_ms.saturating_sub(at));
        if waited.is_some_and(|ms| ms < BUILD_GAP_MS) {
            return Err(TOO_SOON);
        }
        self.last_build_ms = Some(now_ms);
        Ok(())
    }
}
