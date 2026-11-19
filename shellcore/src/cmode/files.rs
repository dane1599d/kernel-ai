//! `:save <name>` and `:load <name>` at `c>` (ADR 0012 D4, from P2).
//!
//! - `:save demo` writes the session's live definitions to
//!   `/home/user/c/demo.c`, a plain C file that starts `#include <kai.h>`:
//!   each entry that defined something, in the order it ran, so a later
//!   redefinition wins as it did at the prompt. The write is one logged file
//!   change, made (`fs_create`) or replaced (`fs_write`), and `undo` at `c>`
//!   puts the file back as it was.
//! - `:load demo` reads that file back and replays it: its definitions,
//!   never its statements, as a restart replays (D4). It is one entry, so
//!   `undo` at `c>` takes its definitions away again, and a restart replays
//!   it with the rest.
//!
//! Both stay in the person's own `/home/user/c/`: a name is a plain word,
//! never a path, so no other folder is read or written.

use super::session::Ran;
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
/// What a saved file starts with.
const HEADER: &str = "#include <kai.h>\n";
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
        let mut text = String::from(HEADER);
        for entry in defined {
            text.push('\n');
            text.push_str(entry.trim_end());
            text.push('\n');
        }
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
        if text.len() > promptcore::MAX_ENTRY {
            return Reply::err(format!(
                "/home/user/{path} is over {} KiB, more than one entry holds",
                promptcore::MAX_ENTRY / 1024
            ));
        }
        let restored = match self.c_ensure_open() {
            Ok(note) => note,
            Err(why) => return Reply::err(why),
        };
        self.c.seq += 1;
        let seq = self.c.seq;
        let request = Request::Replay { seq, entry: &text }.text();
        let reply = match self.host.cc(request.as_bytes()) {
            Ok(reply) => reply,
            Err(why) => {
                self.c.open = false;
                return Reply::err(format!("{restored}cc did not answer: {why}"));
            }
        };
        let records = match parse_reply(&reply) {
            Ok(records) => records,
            Err(e) => return Reply::err(format!("{restored}cc's reply: {e}")),
        };
        let names = records
            .iter()
            .filter(|r| matches!(r, Record::Def { .. }))
            .count();
        match records.last() {
            Some(Record::Done { .. }) => {}
            Some(Record::Err { text: why, .. }) => {
                return Reply::err(format!(
                    "{restored}/home/user/{path} did not load: {}",
                    String::from_utf8_lossy(why)
                ));
            }
            _ => return Reply::err(format!("{restored}cc's reply was malformed")),
        }
        self.c.ran.push(Ran {
            seq,
            defined: Some(text),
            logged: None,
            cc: true,
        });
        Reply::ok(format!(
            "{restored}loaded {names} definitions from /home/user/{path}; statements are not run"
        ))
    }
}
