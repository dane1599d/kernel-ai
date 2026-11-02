//! The editor's file door in the shell (P4): whole files between the editor's
//! window and the user's files. `desktop` carries the bytes here in pieces
//! (`deskcore::files`); this module does the read or the write.
//!
//! A write is one logged action with undo, like any file row's: `fs_write` (the
//! file is replaced) or `fs_create` (it must not exist), the same host change the
//! agent's script makes, its `Inverse::Fs` transaction recorded, and one log
//! entry that holds a short account and never the text. No script engine is
//! involved, so no script, the model's, the panel's or `shell_line`'s, can reach
//! the door: there is no function to call. A model script that names one is
//! refused as it would be for any unknown function.
//!
//! Only the app named [`DOOR_APP`] (the kernel's name for the process, which
//! `desktop` passes on) may use it. A write needs a person's key or click, and is
//! refused when the user withheld the row from the app. Reads and writes are the
//! rows' own tiers: `fs_read`, `fs_write` and `fs_create` run at once, local and
//! undoable, as for an app's effects.

use crate::actions::How;
use crate::actlog::{Form, State};
use crate::grants;
use crate::script::{Checked, file_done, home_path};
use crate::{FsChange, FsRead, Shell};
use agentabi::Tier;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

/// The one app that may use the door: the plan line's name.
pub const DOOR_APP: &str = "editor";
/// The most bytes of a file the door moves.
pub const MAX_BYTES: usize = 256 * 1024;

/// The first bytes of a SHA-256, in hex, for the log's account of a save.
fn short_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

impl Shell {
    /// Why `app` may not use row `row` on `path` through the door; the path as
    /// the `fs` client takes it (relative to `/home/user`) if it may.
    fn door_gate<'a>(&self, app: &str, row: &str, path: &'a str) -> Result<&'a str, String> {
        if app != DOOR_APP {
            return Err(format!("{app} does not use the file door"));
        }
        if grants::is_withheld(self.host.as_ref(), app, row) {
            return Err(format!("{app} does not declare effects that call {row}"));
        }
        // The row's own tier: a file row runs at once only because it can be
        // undone. If that ever changes, the door asks no one and refuses.
        let found =
            agentabi::find(&agentabi::ROWS, row).ok_or_else(|| format!("{row} is not a row"))?;
        if agentabi::derive(found) != Tier::Run || found.facts.open_world || found.facts.destructive
        {
            return Err(format!(
                "{row} asks the user first, which an app cannot do on its own; ask the AI"
            ));
        }
        // The door's paths are absolute, as the editor names them.
        home_path(path)
            .filter(|rest| path.starts_with('/') && !rest.is_empty())
            .ok_or_else(|| format!("{row}: {path} is not under /home/user"))
    }

    /// A file for the editor, whole: its bytes, or why not (missing, not text,
    /// over [`MAX_BYTES`], refused).
    pub fn door_read(&mut self, app: &str, path: &str) -> Result<Vec<u8>, String> {
        let rest = self.door_gate(app, "fs_read", path)?;
        let text = self
            .host
            .fs_read(FsRead::Whole(rest))
            .map_err(|why| door_words("fs_read", path, why))?;
        self.host
            .audit(&format!("{app} read {path} ({} bytes)", text.len()));
        Ok(text.into_bytes())
    }

    /// The editor's save: `bytes` as the whole text of `path`, made when `create`
    /// (it must not exist) and replaced otherwise. One logged action; `undo`
    /// puts the file back exactly as it was.
    pub fn door_write(
        &mut self,
        app: &str,
        path: &str,
        bytes: &[u8],
        create: bool,
        user: bool,
    ) -> Result<(), String> {
        let row = if create { "fs_create" } else { "fs_write" };
        let rest = self.door_gate(app, row, path)?;
        if !user {
            return Err(String::from(
                "a save needs a key or click of the person at the editor",
            ));
        }
        if bytes.len() > MAX_BYTES {
            return Err(format!("a file is at most {} KiB", MAX_BYTES / 1024));
        }
        let text = core::str::from_utf8(bytes).map_err(|_| format!("{path} would not be text"))?;
        let (change, before, after) = match create {
            true => (FsChange::Create { path: rest, text }, "absent", "created"),
            false => (
                FsChange::Write { path: rest, text },
                "its earlier text",
                "written",
            ),
        };
        let txn = self
            .host
            .fs_change(change)
            .map_err(|why| door_words(row, path, why))?;
        let done = file_done(row, rest, before, after, Some(txn));
        // The log holds a short account, never the text: what was saved is told
        // by its size and a hash, and the file's earlier text is the undo's.
        let id = self.next_id;
        self.next_id += 1;
        let request = format!(
            "{app}: save {path} ({} bytes, sha256 {})",
            bytes.len(),
            short_hash(bytes)
        );
        let call = format!("{row}(\"{rest}\", <{} bytes>)", bytes.len());
        let how = How {
            form: Form::Call,
            tier: Tier::Run,
            consent: format!("app:{app}"),
        };
        self.log_entry(
            id,
            &request,
            &Checked::shown_only(&call),
            how,
            State::Done,
            "ran",
            alloc::vec![done],
        );
        // The log line is written into the open action, so the change and its
        // line land in one commit.
        self.save_log();
        Ok(())
    }
}

/// A refusal from the file service in the person's words. A path that is not
/// there starts `missing`, which the editor reads as "make a new file".
fn door_words(row: &str, path: &str, why: String) -> String {
    if why.contains("NotFound") {
        return format!("missing: {path} is not there");
    }
    if why.contains("Exists") {
        return format!("{path} exists already; a new file never overwrites");
    }
    format!("{row}: {why}")
}
