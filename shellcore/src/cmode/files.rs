//! `:save <name>` and `:load <name>` at `c>` (ADR 0012 D4, from P2).
//!
//! - `:save demo` writes the session to `/home/user/c/demo.c`, a plain C
//!   file that starts `#include <kai.h>`: each entry that defined something,
//!   as typed and in the order it ran, after a mark line (`saved.rs`). The
//!   write is one logged file change, made (`fs_create`) or replaced
//!   (`fs_write`), and `undo` at `c>` puts the file back as it was.
//! - `:load demo` replays that file's entries one at a time, as the session
//!   ran them: their definitions, never their statements, as a restart
//!   replays (D4), so a redefinition replaces as it did when typed. Each is
//!   an entry of the session, which `undo` at `c>` takes back as a typed one
//!   and a restart replays; a file that fails part way is taken back whole.
//! - `#include "…"` of a saved file reads it as one unit, so one that
//!   defines a name twice is refused by name: `:load` it instead.
//!
//! Both stay in the person's own `/home/user/c/`: a name is a plain word,
//! never a path, so no other folder is read or written.

use super::saved;
use super::session::{Ran, no_session};
use crate::actions::How;
use crate::actlog::{Form, State};
use crate::script::{Checked, file_done};
use crate::{FsChange, FsRead, Reply, Shell};
use agentabi::Tier;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use promptcore::wire::{Record, Request, parse_reply};

/// The folder under `/home/user` that `:save` and `:load` use.
const FOLDER: &str = "c";
/// The longest name `:save` and `:load` take.
const MAX_NAME: usize = 32;

/// `name`'s file, relative to `/home/user` as the `fs` client takes it: a
/// name is 1 to 32 letters, digits, `_` or `-`, starting with a letter, so
/// it never names another folder.
fn file_of(name: &str) -> Result<String, String> {
    let plain = name.len() <= MAX_NAME
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    match plain {
        true => Ok(format!("{FOLDER}/{name}.c")),
        false => Err(format!(
            "a name is 1 to {MAX_NAME} letters, digits, _ or -, starting with a letter: :save demo writes /home/user/c/demo.c"
        )),
    }
}

/// A file's words for the person, as the file door says them.
fn missing_or(path: &str, why: &str) -> String {
    match why.contains("NotFound") {
        true => format!("/home/user/{path} is not there"),
        false => format!("/home/user/{path}: {why}"),
    }
}

/// The file an `#include "<path>"` names, relative to `/home/user` as the
/// `fs` client takes it: a relative path is in `/home/user/c/`, an absolute
/// one must be under `/home/user`, and neither may climb out with `..`.
fn include_of(path: &str) -> Result<String, String> {
    let whole = match path.starts_with('/') {
        true => String::from(path),
        false => format!("/home/user/{FOLDER}/{path}"),
    };
    let plain = !path.is_empty() && whole.split('/').skip(1).all(|part| !part.is_empty());
    match crate::script::home_path(&whole) {
        Some(rest) if plain && !rest.is_empty() => Ok(String::from(rest)),
        _ => Err(format!(
            "#include \"{path}\": only a file in your own folders, under /home/user"
        )),
    }
}

/// The SHA-256 of `expanded`, what cc ran for the entry `typed`, when its
/// `#include "…"` files made it other than the text: kept with the entry, so
/// a replay of files changed since is refused.
pub(super) fn included(typed: &str, expanded: &str) -> Option<[u8; 32]> {
    use sha2::{Digest, Sha256};
    (typed != expanded).then(|| Sha256::digest(expanded.as_bytes()).into())
}

/// The path of an `#include "<path>"` line, or `None` for any other line.
fn quoted_include(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("include")?.trim_start();
    rest.strip_prefix('"')?.strip_suffix('"')
}

impl Shell {
    /// `text` with each `#include "<path>"` line at its top replaced by the
    /// file's text, read through the `fs` rows from the person's own folders
    /// only (ADR 0012 D4, under the line's own grant: the typed line's, the
    /// person's). An included file's own `#include "…"` lines are not read.
    pub(super) fn c_includes(&self, text: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut lines = text.split_inclusive('\n').peekable();
        while let Some(line) =
            lines.next_if(|line| line.trim().is_empty() || quoted_include(line).is_some())
        {
            let Some(path) = quoted_include(line) else {
                out.push_str(line);
                continue;
            };
            let file = include_of(path)?;
            let included = self
                .host
                .fs_read(FsRead::Whole(&file))
                .map_err(|why| format!("#include \"{path}\": {}", missing_or(&file, &why)))?;
            self.host.audit(&format!(
                "c> read /home/user/{file} for #include ({} bytes)",
                included.len()
            ));
            // One unit cannot define a name twice: a saved session that
            // redefined one loads, entry by entry, but does not include.
            if let Some(name) = saved::redefined(&included) {
                return Err(format!(
                    "#include \"{path}\": it defines {name} twice; use :load"
                ));
            }
            out.push_str(included.trim_end());
            out.push('\n');
        }
        out.extend(lines);
        Ok(out)
    }

    /// `:save <name>` (see the module header).
    pub(super) fn c_save(&mut self, name: &str) -> Reply {
        let path = match file_of(name) {
            Ok(path) => path,
            Err(why) => return Reply::err(why),
        };
        let defined: Vec<&str> = self
            .c
            .ran
            .iter()
            .filter_map(|ran| ran.defined.as_deref())
            .collect();
        let entries = defined.len();
        if entries == 0 {
            return Reply::err("nothing is defined yet, so there is nothing to save");
        }
        let text = match saved::file(&defined) {
            Ok(text) => text,
            Err(why) => return Reply::err(format!("not saved: {why}")),
        };
        let exists = self.host.fs_read(FsRead::Whole(&path)).is_ok();
        let (row, before, after) = match exists {
            true => ("fs_write", "its earlier text", "written"),
            false => ("fs_create", "absent", "created"),
        };
        // The folder is made too when it is missing, in the same transaction.
        let change = FsChange::Save {
            folder: FOLDER,
            path: &path,
            text: &text,
        };
        let txn = match self.host.fs_change(change) {
            Ok(txn) => txn,
            Err(why) => return Reply::err(missing_or(&path, &why)),
        };
        let id = self.next_id;
        self.next_id += 1;
        let request = format!(":save {name}");
        let call = format!("{row}(\"{path}\", <{} bytes>)", text.len());
        let how = How {
            form: Form::Call,
            tier: Tier::Run,
            consent: String::from("user"),
        };
        let done = file_done(row, &path, before, after, Some(txn));
        self.log_entry(
            id,
            &request,
            &Checked::shown_only(&call),
            how,
            State::Done,
            "ran",
            alloc::vec![done],
        );
        self.save_log();
        // `undo` at c> reverts it: an entry with a log entry and no cc step.
        self.c.seq += 1;
        self.c.ran.push(Ran {
            seq: self.c.seq,
            defined: None,
            logged: Some(id),
            cc: false,
            included: None,
        });
        Reply::ok(format!(
            "saved {entries} entries to /home/user/{path}; undo puts it back"
        ))
    }

    /// `:load <name>` (see the module header).
    pub(super) fn c_load(&mut self, name: &str) -> Reply {
        let path = match file_of(name) {
            Ok(path) => path,
            Err(why) => return Reply::err(why),
        };
        let text = match self.host.fs_read(FsRead::Whole(&path)) {
            Ok(text) => text,
            Err(why) => return Reply::err(missing_or(&path, &why)),
        };
        self.host
            .audit(&format!("c> read /home/user/{path} ({} bytes)", text.len()));
        let entries = saved::entries(&text);
        if let Some(n) = entries.iter().position(|e| e.len() > promptcore::MAX_ENTRY) {
            return Reply::err(format!(
                "/home/user/{path}: entry {} is over {} KiB, more than one entry holds",
                n + 1,
                promptcore::MAX_ENTRY / 1024
            ));
        }
        let mut restored = match self.c_ensure_open() {
            Ok(note) => note,
            Err(why) => return Reply::err(why),
        };
        let mut names = 0;
        for (n, entry) in entries.iter().enumerate() {
            match self.c_replay(entry) {
                Ok((defined, note)) => {
                    names += defined;
                    restored.push_str(&note);
                }
                Err(why) => {
                    self.c_unload(n);
                    return Reply::err(format!(
                        "{restored}/home/user/{path} did not load (entry {}): {why}",
                        n + 1
                    ));
                }
            }
        }
        Reply::ok(format!(
            "{restored}loaded {} entries ({names} definitions) from /home/user/{path}; statements are not run",
            entries.len()
        ))
    }

    /// Loads `entry`'s definitions and their initializers as one entry of
    /// the session, as it first ran without its statements: how many it made,
    /// or why not (cc then committed nothing of it). It is kept as typed;
    /// only cc sees its `#include "…"` files.
    fn c_replay(&mut self, entry: &str) -> Result<(usize, String), String> {
        let expanded = self.c_includes(entry)?;
        if expanded.len() > promptcore::MAX_ENTRY {
            return Err(format!(
                "with its #include files it is over {} KiB",
                promptcore::MAX_ENTRY / 1024
            ));
        }
        self.c.seq += 1;
        let seq = self.c.seq;
        let request = Request::Load {
            seq,
            entry: &expanded,
        }
        .text();
        let mut note = String::new();
        let mut reply = self.c_send(&request)?;
        // A cc the supervisor restarted since the last line has no session:
        // open it again, replay, and send this once more, as an entry does.
        if no_session(&reply) {
            self.c.open = false;
            note = self.c_ensure_open()?;
            reply = self.c_send(&request)?;
        }
        let Ok(records) = parse_reply(&reply) else {
            // Whether cc committed it is unknown: the next entry opens a
            // session again and replays only what `ran` holds.
            self.c.open = false;
            return Err(String::from("cc's reply did not parse"));
        };
        let names = records
            .iter()
            .filter(|r| matches!(r, Record::Def { .. }))
            .count();
        match records.last() {
            Some(Record::Done { .. }) => {}
            Some(Record::Err { text: why, .. }) => {
                return Err(String::from_utf8_lossy(why).into_owned());
            }
            // An initializer that calls kai stops at a need; a load runs no
            // call, so the entry fails and the load is taken back.
            // cc drops the run waiting on it at the next request, the
            // take-back's undo, and commits nothing of it: the session stays.
            Some(Record::Need { .. }) => {
                return Err(String::from(
                    "an initializer calls kai, and :load runs no calls",
                ));
            }
            _ => {
                self.c.open = false;
                return Err(String::from("cc's reply was malformed"));
            }
        }
        self.c.ran.push(Ran {
            seq,
            defined: Some(String::from(entry)),
            logged: None,
            cc: true,
            included: included(entry, &expanded),
        });
        Ok((names, note))
    }

    /// One request to cc; when it does not answer, the session is closed.
    fn c_send(&mut self, request: &str) -> Result<Vec<u8>, String> {
        self.host.cc(request.as_bytes()).map_err(|why| {
            self.c.open = false;
            format!("cc did not answer: {why}")
        })
    }

    /// Takes back the `count` entries a failed `:load` replayed, last first,
    /// so a file loads whole or not at all.
    fn c_unload(&mut self, count: usize) {
        for _ in 0..count {
            self.c.ran.pop();
            if !self.c.open {
                continue;
            }
            let undone = self
                .host
                .cc(Request::Undo.text().as_bytes())
                .is_ok_and(|reply| {
                    matches!(parse_reply(&reply).as_deref(), Ok([Record::Undone(_)]))
                });
            if !undone {
                // cc lost its journal, or its reply did not parse: no more
                // undo goes to it, and the next entry opens a session again
                // and replays what is left.
                self.c.open = false;
            }
        }
    }
}
