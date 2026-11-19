//! The memory store's keeping (M2): load at start, save after each change,
//! purge after 10 minutes, the log entries, `undo`, and the memories the
//! State carries.
//!
//! - **An edit's earlier words** stay in the shell, never in the log, whose
//!   one-word fields would read them back changed: `undo` of an edit finds
//!   them here while the memory still says what the edit wrote. After a
//!   restart they are gone, and `undo` says so instead of guessing.
//! - **The State** carries at most 3 memories for everyone that share a word
//!   with the request, in plain ASCII, inside the room the line leaves
//!   (`state::with_memory`). They inform the model and never give consent.

use crate::Shell;
use crate::actlog::{Change, Entry, Form, State};
use crate::undo::Inverse;
use agentabi::Tier;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kaimem::record::Scope;
use kaimem::store::Who;

/// The most edits whose earlier words `undo` can find.
const MAX_EDITS: usize = 32;

/// An edit of memory `memory` from `before` to `after`, for its `undo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edited {
    memory: u64,
    after: String,
    before: String,
}

impl Shell {
    /// The memories `memory_read` holds, loaded at start; records out of
    /// shape are skipped and said.
    pub(crate) fn with_saved_memories(mut self) -> Self {
        let Some(text) = self.host.memory_read() else {
            return self;
        };
        match kaimem::store::Memories::load(&text) {
            Ok(loaded) => {
                for skipped in &loaded.skipped {
                    self.host
                        .audit(&format!("a memory record was skipped: {skipped:?}"));
                }
                self.memories = loaded.memories;
            }
            Err(why) => self.host.audit(&format!(
                "the memory file is out of shape, so none is read: {why}"
            )),
        }
        self
    }

    /// Keeps an edit's earlier words for its `undo`.
    pub(crate) fn memory_edited(&mut self, memory: u64, after: &str, before: &str) {
        if self.memory_edits.len() >= MAX_EDITS {
            self.memory_edits.remove(0);
        }
        self.memory_edits.push(Edited {
            memory,
            after: String::from(after),
            before: String::from(before),
        });
    }

    pub(crate) fn memory_logged(
        &mut self,
        id: u64,
        request: &str,
        row: &str,
        consent: &str,
        changes: Vec<Change>,
    ) {
        let script = changes
            .first()
            .map(|c| format!("{row}({})", c.res.trim_start_matches("memory ")))
            .unwrap_or_default();
        self.log.push(Entry {
            agent: agentid::Name::kai(),
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: Some(if consent == "yes" {
                Tier::Ask
            } else {
                Tier::Run
            }),
            pack: None,
            consent: String::from(consent),
            state: State::Done,
            request: String::from(request),
            script,
            outcome: String::from("ran"),
            changes,
        });
        self.save_log();
        self.memory_save();
    }

    /// Drops memories forgotten more than 10 minutes ago, for good.
    pub(crate) fn memory_purge(&mut self) {
        if self.memories.purge(self.host.now_ns()) > 0 {
            self.memory_save();
        }
    }

    fn memory_save(&mut self) {
        if let Err(why) = self.host.memory_write(&self.memories.save()) {
            self.host
                .audit(&format!("the memories were not saved: {why}"));
        }
    }

    /// `undo` of a memory change: `MemoryRestore` brings a forgotten one
    /// back (within 10 minutes), `MemoryForget` forgets one again, and
    /// `MemoryText` puts an edit's earlier words back while the memory still
    /// says what the edit wrote.
    pub(crate) fn revert_memory(&mut self, inverse: Inverse) -> Result<String, String> {
        let now = self.host.now_ns();
        let done = match inverse {
            Inverse::MemoryRestore { id } => self
                .memories
                .restore(Who::User, id, now)
                .map(|()| format!("memory {id} is back")),
            Inverse::MemoryForget { id } => self
                .memories
                .forget(Who::User, id, now)
                .map(|()| format!("memory {id} forgotten")),
            Inverse::MemoryText { id } => self.memory_text_back(id, now),
            _ => return Err(String::from("not a memory change")),
        };
        if done.is_ok() {
            self.memory_save();
        }
        done
    }

    /// Memory `id`'s words before its last edit, back, and that edit kept
    /// the other way round for a redo.
    fn memory_text_back(&mut self, id: u64, now: u64) -> Result<String, String> {
        let says = self
            .memories
            .get(Who::User, id)
            .map(|m| m.text.clone())
            .ok_or_else(|| format!("memory {id} is not kept"))?;
        let at = self
            .memory_edits
            .iter()
            .rposition(|e| e.memory == id && e.after == says)
            .ok_or_else(|| format!("memory {id}'s earlier words are not kept since the restart"))?;
        let edit = self.memory_edits.remove(at);
        self.memories.edit(Who::User, id, &edit.before, now)?;
        self.memory_edited(id, &edit.before, &edit.after);
        Ok(format!("memory {id} says \"{}\" again", edit.before))
    }

    /// The State the next request `request` carries: the OS's, with the
    /// memories for everyone, in plain ASCII, that share a word with it, in
    /// the room the line has left.
    pub(crate) fn state_line_for(&self, request: &str) -> Option<String> {
        let state = self.host.state();
        let memories: Vec<&kaimem::record::Memory> = self
            .memories
            .list(Who::User)
            .into_iter()
            .filter(|m| m.scope == Scope::All && m.text.is_ascii())
            .collect();
        let memory = kaimem::prompt::state(&memories, request, state.memory_room());
        state.snapshot_with(&memory)
    }
}
