//! The session's builds (M9d): what `status` shows, and the last build's answer.

use super::{Service, ok};
use alloc::format;
use alloc::string::String;

/// What the service knows of its builds.
pub(super) struct Builds {
    /// The answer the last build gave, once it finished.
    pub(super) last: Option<String>,
}

impl Builds {
    pub(super) const fn new() -> Self {
        Self { last: None }
    }
}

impl Service {
    /// `status`: the first line is `ok <component> <label>: unchanged|edited[,
    /// built <label>]`, or `ok no file is open`; a `last build: <answer>` line
    /// follows once a build has finished.
    pub(super) fn status(&self) -> String {
        let mut out = match &self.session {
            Some(session) => {
                let state = if session.changed() {
                    "edited"
                } else {
                    "unchanged"
                };
                let built = match &session.built {
                    Some(built) => format!(", built {built}"),
                    None => String::new(),
                };
                ok(format!(
                    "{} {}: {state}{built}",
                    session.component, session.label
                ))
            }
            None => ok("no file is open"),
        };
        if let Some(last) = &self.builds.last {
            out.push_str("\nlast build: ");
            out.push_str(last);
        }
        out
    }
}
