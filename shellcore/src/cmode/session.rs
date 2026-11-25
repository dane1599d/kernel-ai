//! The C session in cc, from the shell's side: opening it, and replaying the
//! definitions after cc restarts (ADR 0012 D4: definitions, never statements,
//! and the reply says so); `:dis`, `:defs` and `:reset`; and `undo`.
//!
//! `undo` at `c>` reverts the last entry that defined or changed something.
//! The log goes first: if a later change touched the same thing, nothing is
//! undone. Then cc goes back one entry at a time to it, entries that only ran
//! code included, so cc's journal and the shell's stay aligned.

use super::saved;
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use promptcore::defs::Kind;
use promptcore::wire::{Record, Request, parse_reply};

/// An entry that ran: what `undo` and a replay need.
#[derive(Clone, Debug)]
pub(super) struct Ran {
    pub(super) seq: u64,
    /// Its text, when it defined something: replayed after a restart.
    pub(super) defined: Option<String>,
    /// The log entry its calls' changes joined.
    pub(super) logged: Option<u64>,
    /// It is in cc's journal of the user's session: Kai's program is not.
    pub(super) cc: bool,
    /// The SHA-256 of what cc ran, when its `#include "…"` files made that
    /// other than its text: a replay of files changed since is refused.
    pub(super) included: Option<[u8; 32]>,
    /// The names what cc ran defines, `#include` files read as they were
    /// then (keys as [`saved::defines`] gives them): a restart that skips the
    /// entry still knows what it defined, though the files may be gone.
    pub(super) names: Vec<String>,
    /// Of those, the functions cc redefined in place, same type, so every
    /// caller already defined follows (what the replies' `def` and
    /// `retyped` records say, not a guess from the text). A global, a retyped
    /// function, a macro or a tag is not here: earlier callers keep the old.
    pub(super) rebinds: Vec<String>,
}

/// The functions in `records` that cc defined in place: a function `def`
/// without a `retyped` for it.
pub(super) fn rebound(records: &[Record]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for record in records {
        match record {
            Record::Def {
                kind: Kind::Func,
                name,
                ..
            } => names.push(name.to_string()),
            Record::Retyped(name) => names.retain(|n| n != name),
            _ => {}
        }
    }
    names
}

/// One entry of a restart, in the order they ran.
struct Step {
    seq: u64,
    /// What it defined when it ran.
    names: Vec<String>,
    /// The part of `names` defined in place: [`Ran::rebinds`].
    rebinds: Vec<String>,
    /// Its text as cc replays it, when it was restored.
    restored: Option<String>,
}

/// The notes for restored entries that mention a name a skipped entry
/// defined. A mention is a match on the word, so it may be a parameter or a
/// local: each line says what the session knows, no more.
///
/// What a caller ends up bound to is found the way the linker did it: from
/// the nearest definer before the caller, forward through each later definer
/// that redefined in place (a slot every caller shares), and no further. A
/// global, a retyped function, a macro or a tag is bound where the caller
/// is. The name is stale when that last definer was skipped. A caller with
/// no definer before it is not named, and nor is one whose last definer was
/// restored, though a retyped chain may differ: silence, never a guess.
fn rebind_notes(steps: &[Step]) -> String {
    let mut skipped: Vec<String> = Vec::new();
    for step in steps.iter().filter(|s| s.restored.is_none()) {
        for name in &step.names {
            if !skipped.contains(name) {
                skipped.push(name.clone());
            }
        }
    }
    let mut out = String::new();
    for (at, step) in steps.iter().enumerate() {
        let Some(text) = &step.restored else {
            continue;
        };
        for key in saved::mentions(text, &skipped) {
            let by = |s: &&Step| s.names.contains(&key);
            let Some(first) = steps[..at].iter().rposition(|s| by(&s)) else {
                continue;
            };
            let mut last = first;
            for (i, later) in steps.iter().enumerate().skip(at + 1) {
                if !by(&later) {
                    continue;
                }
                if !later.rebinds.contains(&key) {
                    break;
                }
                last = i;
            }
            let stale = steps[last].restored.is_none();
            let live = steps[..at].iter().any(|s| s.restored.is_some() && by(&s));
            if !stale {
                continue;
            }
            let (seq, name) = (step.seq, saved::bare(&key));
            out.push_str(&match live {
                true => format!(
                    "entry {seq} mentions {name}, which a skipped entry redefined; if it means that one, it now uses the earlier definition\n"
                ),
                false => format!(
                    "entry {seq} mentions {name}, which a skipped entry defined and no restored entry defines now\n"
                ),
            });
        }
    }
    out
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
        let kept: Vec<Ran> = self
            .c
            .ran
            .iter()
            .filter(|r| r.defined.is_some())
            .cloned()
            .collect();
        if kept.is_empty() {
            return Ok(String::new());
        }
        // cc's journal starts again: undo cannot go back past a restart.
        self.c.ran.clear();
        let mut restored = 0;
        let mut skipped = String::new();
        let mut steps: Vec<Step> = Vec::new();
        for ran in kept {
            let (seq, names, included) = (ran.seq, ran.names, ran.included);
            let ran_rebinds = ran.rebinds;
            let text = ran.defined.unwrap_or_default();
            let step = |restored, rebinds| Step {
                seq,
                names: names.clone(),
                rebinds,
                restored,
            };
            // An entry is kept as typed: its `#include "…"` files are read
            // again, and one that is gone or changed since is not restored,
            // and the person is told, never given code they did not run.
            let entry = match self.c_includes(&text) {
                Ok(entry) => entry,
                Err(why) => {
                    skipped.push_str(&format!("entry {seq}'s {why}; not restored\n"));
                    steps.push(step(None, ran_rebinds.clone()));
                    continue;
                }
            };
            if included.is_some_and(|hash| Some(hash) != super::files::included(&text, &entry)) {
                skipped.push_str(&format!(
                    "entry {seq}'s #include files changed since it ran; not restored\n"
                ));
                steps.push(step(None, ran_rebinds.clone()));
                continue;
            }
            let replay = Request::Replay { seq, entry: &entry }.text();
            // An entry that does not replay is named too.
            match self.c_replayed(&replay) {
                Err(why) => {
                    skipped.push_str(&format!(
                        "entry {seq} did not replay: {}; not restored\n",
                        why.trim_end()
                    ));
                    steps.push(step(None, ran_rebinds));
                }
                Ok(rebinds) => {
                    steps.push(step(Some(entry), rebinds.clone()));
                    restored += 1;
                    self.c.ran.push(Ran {
                        seq,
                        defined: Some(text),
                        logged: None,
                        cc: true,
                        included,
                        names,
                        rebinds,
                    });
                }
            }
        }
        skipped.push_str(&rebind_notes(&steps));
        Ok(format!(
            "cc restarted; restored {restored} definitions; globals reset\n{skipped}"
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

    /// Sends a replay: `Ok` when cc ran it to its end, or why not.
    fn c_replayed(&self, request: &str) -> Result<Vec<String>, String> {
        let reply = self
            .host
            .cc(request.as_bytes())
            .map_err(|why| format!("cc did not answer: {why}"))?;
        match parse_reply(&reply).as_deref() {
            Ok(records @ [.., Record::Done { .. }]) => Ok(rebound(records)),
            Ok([.., Record::Err { text, .. }]) => Err(String::from_utf8_lossy(text).into_owned()),
            _ => Err(String::from("cc's reply was malformed")),
        }
    }

    /// `:dis f` and `:defs`: cc's text.
    pub(super) fn c_command(&mut self, request: &Request) -> Reply {
        // A restart's note goes in front of the answer, as for an entry.
        let note = match self.c_ensure_open() {
            Ok(note) => note,
            Err(why) => return Reply::err(why),
        };
        let reply = self.c_command_text(request);
        Reply {
            text: format!("{note}{}", reply.text),
            ..reply
        }
    }

    fn c_command_text(&mut self, request: &Request) -> Reply {
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
        let steps = self.c.ran[at..].iter().filter(|r| r.cc).count();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn step(seq: u64, names: &[&str], restored: Option<&str>) -> Step {
        Step {
            seq,
            names: names.iter().map(|n| String::from(*n)).collect(),
            rebinds: Vec::new(),
            restored: restored.map(String::from),
        }
    }

    /// A macro binds where its caller is defined: an entry before the
    /// skipped definition is not named, one after it is (#1266's review).
    #[test]
    fn a_macro_caller_is_named_only_after_the_skipped_definition() {
        let steps = [
            step(1, &["#N"], Some("#define N 2")),
            step(2, &[], Some("I64 a[N];")),
            step(3, &["#N"], None),
            step(4, &[], Some("I64 b[N];")),
        ];
        let notes = rebind_notes(&steps);
        assert!(notes.contains("entry 4 mentions N, which a skipped entry redefined"));
        assert!(!notes.contains("entry 2"), "{notes}");
    }
}
