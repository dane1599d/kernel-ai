//! `kai-actions 1` as a request: the agent core asks an app for its action
//! rows (P4, the agent-to-app route). The answer is the `kai.actions` section
//! of the artifact the host loaded, which the kernel's store checked, never
//! anything the app's code makes at run time:
//!
//! ```text
//! version 2
//! kai-actions 1
//! world app:notes/notes@1.0.0
//! action notes_add
//! ...
//! ```
//!
//! or `err <why>` on one line.

use crate::artifact;
use crate::host::{Env, Host, Mode};
use alloc::format;
use alloc::string::String;

pub const REQUEST: &str = "kai-actions 1";
/// The longest answer: one message.
pub const MAX_REPLY: usize = kabi::MAX_MESSAGE;

/// The answer for version `label`, loaded from `artifact`.
pub fn reply(label: &str, artifact: &[u8]) -> String {
    let rows = match artifact::parse(artifact) {
        Ok(parsed) => parsed.actions,
        Err(why) => return format!("err its artifact does not parse: {why}"),
    };
    let Some(rows) = rows else {
        return String::from("err it declares no actions");
    };
    let text = format!("version {label}\n{rows}");
    match text.len() <= MAX_REPLY {
        true => text,
        false => String::from("err its actions are over one message"),
    }
}

/// Whether `request` asks for the rows.
pub fn is_request(request: &[u8]) -> bool {
    request.strip_suffix(b"\n").unwrap_or(request) == REQUEST.as_bytes()
}

impl<E: Env> Host<E> {
    /// The active version's rows: the serving one's, a probation's standby's,
    /// a suspended or disabled app's. A suspended app stays suspended.
    pub(crate) fn rows(&self) -> String {
        let (label, artifact) = match &self.mode {
            Mode::Serving(version) | Mode::Disabled { version, .. } => {
                (&version.label, &version.artifact)
            }
            Mode::Probation(probation) => (&probation.standby.label, &probation.standby.artifact),
            Mode::Suspended(parked) => (&parked.label, &parked.artifact),
            Mode::Moving => return String::from("err busy"),
        };
        reply(label, artifact)
    }
}
