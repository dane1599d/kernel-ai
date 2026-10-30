//! The C session in cc, from the shell's side: opening it, and replaying the
//! definitions after cc restarts (ADR 0012 D4: definitions, never statements,
//! and the reply says so); `:dis`, `:defs` and `:reset`; and `undo`.
//!
//! `undo` at `c>` reverts the last entry that defined or changed something.
//! The log goes first: if a later change touched the same thing, nothing is
//! undone. Then cc goes back one entry at a time to it, entries that only ran
//! code included, so cc's journal and the shell's stay aligned.

use crate::{Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use promptcore::wire::{Record, Request, parse_reply};

/// An entry that ran: what `undo` and a replay need.
#[derive(Clone, Debug)]
pub(super) struct Ran {
    pub(super) seq: u64,
    /// Its text, when it defined something: replayed after a restart.
    pub(super) defined: Option<String>,
    /// The log entry its calls' changes joined.
    pub(super) logged: Option<u64>,
}

impl Shell {
    /// Opens cc's session if it is not open, replaying the definitions kept
    /// from before a restart; a note saying what was restored.
    pub(super) fn c_ensure_open(&mut self) -> Result<String, String> {
        if self.c.open {
            return Ok(String::new());
        }
        let open = Request::Open {
            session: "main",
            origin: promptcore::wire::Origin::User,
        };
        self.c_expect_ok(&open.text())?;
        self.c.open = true;
        let kept: Vec<(u64, String)> = self
            .c
            .ran
            .iter()
            .filter_map(|r| r.defined.clone().map(|text| (r.seq, text)))
            .collect();
        if kept.is_empty() {
            return Ok(String::new());
        }
        // cc's journal starts again: undo cannot go back past a restart.
        self.c.ran.clear();
        let mut restored = 0;
        for (seq, text) in kept {
            let replay = Request::Replay { seq, entry: &text }.text();
            if self.c_expect_done(&replay) {
                restored += 1;
                self.c.ran.push(Ran {
                    seq,
                    defined: Some(text),
                    logged: None,
                });
            }
        }
        Ok(format!(
            "cc restarted; restored {restored} definitions; globals reset\n"
        ))
    }

    fn c_expect_ok(&self, request: &str) -> Result<(), String> {
        let reply = self.host.cc(request.as_bytes())?;
        match parse_reply(&reply).map_err(|e| e.to_string())?.as_slice() {
            [Record::Ok] => Ok(()),
            [.., Record::Err { text, .. }] => Err(String::from_utf8_lossy(text).into_owned()),
            _ => Err(String::from("cc's reply was not ok")),
        }
    }

    fn c_expect_done(&self, request: &str) -> bool {
        let Ok(reply) = self.host.cc(request.as_bytes()) else {
            return false;
        };
        matches!(
            parse_reply(&reply).as_deref(),
            Ok([.., Record::Done { .. }])
        )
    }

    /// `:dis f` and `:defs`: cc's text.
    pub(super) fn c_command(&mut self, request: &Request) -> Reply {
        if let Err(why) = self.c_ensure_open() {
            return Reply::err(why);
        }
        let reply = match self.host.cc(request.text().as_bytes()) {
            Ok(reply) => reply,
            Err(why) => {
                self.c.open = false;
                return Reply::err(format!("cc did not answer: {why}"));
            }
        };
        match parse_reply(&reply).as_deref() {
            Ok([Record::Text([])]) => Reply::ok("nothing defined yet"),
            Ok([Record::Text(text)]) => Reply::ok(String::from_utf8_lossy(text).trim_end()),
            Ok([.., Record::Err { text, .. }]) => Reply::err(String::from_utf8_lossy(text)),
            _ => Reply::err("cc's reply was malformed"),
        }
    }

    /// `:reset`: a new session; what was defined is gone, and nothing replays.
    pub(super) fn c_reset(&mut self) -> Reply {
        self.c.ran.clear();
        self.c.open = false;
        match self.c_ensure_open() {
            Ok(_) => Reply::ok("the C session is empty again"),
            Err(why) => Reply::err(why),
        }
    }

    /// `undo` at `c>` (see the module header).
    pub(super) fn c_undo(&mut self) -> Reply {
        let Some(at) = self
            .c
            .ran
            .iter()
            .rposition(|r| r.defined.is_some() || r.logged.is_some())
        else {
            return Reply::err("nothing to undo at c>");
        };
        let target = self.c.ran[at].clone();
        let mut text = String::new();
        if let Some(id) = target.logged {
            let reverted = self.undo_command(&id.to_string());
            // A question about putting back anyway is answered at kai>, not
            // by the next C line.
            if self.undo_asking.take().is_some() {
                return Reply::err(format!(
                    "entry {} was not undone: {}; undo #{id} at kai> to choose",
                    target.seq, reverted.text
                ));
            }
            if !reverted.ok {
                return Reply::err(format!(
                    "entry {} was not undone: {}",
                    target.seq, reverted.text
                ));
            }
            text = reverted.text;
        }
        let steps = self.c.ran.len() - at;
        self.c.ran.truncate(at);
        let mut cc_ok = !self.c.open;
        for _ in 0..steps {
            if !self.c.open {
                break;
            }
            let reply = self.host.cc(Request::Undo.text().as_bytes());
            cc_ok = matches!(reply.as_deref().map(parse_reply), Ok(Ok(ref r)) if matches!(r.as_slice(), [Record::Undone(_)]));
            if !cc_ok {
                // cc lost its journal: the next entry opens a session again
                // and replays what is left.
                self.c.open = false;
            }
        }
        if target.defined.is_some() {
            let gone = match cc_ok {
                true => "its definitions are gone",
                false => "its definitions are gone once cc replays",
            };
            text = match text.is_empty() {
                true => String::from(gone),
                false => format!("{gone}; {text}"),
            };
        }
        Reply::ok(format!("undid entry {}: {text}", target.seq))
    }
}

/// Whether cc's reply says it has no session: it restarted.
pub(super) fn no_session(reply: &[u8]) -> bool {
    matches!(
        parse_reply(reply).as_deref(),
        Ok([Record::Err {
            text: b"no session is open",
            ..
        }])
    )
}

/// The `resume` for a call's value.
pub(super) fn resume(need: u64, value: Result<serde_json::Value, String>) -> String {
    let (ok, result) = match value {
        Ok(value) => (true, value.to_string()),
        Err(why) => (false, why),
    };
    let request = Request::Resume {
        id: need,
        ok,
        result: &result,
    }
    .text();
    // One message holds at most 8 KiB; a value too long for it fails the call.
    match request.len() <= kabi::MAX_MESSAGE {
        true => request,
        false => Request::Resume {
            id: need,
            ok: false,
            result: "the result is too long for c>",
        }
        .text(),
    }
}
