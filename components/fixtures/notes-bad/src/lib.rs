//! `notes` version 2 with exactly one real bug, picked by a cargo feature:
//!
//! - `bug-import`: calls a host function that does not exist, so it cannot link;
//! - `bug-health`: its migration forgets the word counts, so the health check fails;
//! - `bug-trap`: keeps the last six ids in a fixed array and overruns it on the
//!   seventh note;
//! - `bug-spin`: its `find` loop only advances on a match, so a word that is not in
//!   the first note loops forever.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use comp_notes::Notes;
use compsdk::{Component, ImportError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bug {
    None,
    Import,
    Health,
    Trap,
    Spin,
}

pub const BUG: Bug = if cfg!(feature = "bug-import") {
    Bug::Import
} else if cfg!(feature = "bug-health") {
    Bug::Health
} else if cfg!(feature = "bug-trap") {
    Bug::Trap
} else if cfg!(feature = "bug-spin") {
    Bug::Spin
} else {
    Bug::None
};

const _: () = assert!(
    cfg!(feature = "bug-import") as u8
        + cfg!(feature = "bug-health") as u8
        + cfg!(feature = "bug-trap") as u8
        + cfg!(feature = "bug-spin") as u8
        <= 1,
    "build notes-bad with one bug at a time"
);

#[cfg(target_family = "wasm")]
#[link(wasm_import_module = "kai")]
unsafe extern "C" {
    fn teleport();
}

pub struct Bad {
    pub notes: Notes,
    recent: [u32; 6],
    bug: Bug,
}

impl Bad {
    pub fn with(mut notes: Notes, bug: Bug) -> Self {
        if bug == Bug::Health {
            for note in &mut notes.state.notes {
                note.words = 0;
            }
        }
        Self {
            notes,
            recent: [0; 6],
            bug,
        }
    }

    /// `find` with the spin bug: the index moves only when a note matches. Gives up
    /// after `limit` steps when one is set, so the host test can watch it loop.
    pub fn find_spinning(&self, word: &str, limit: Option<u64>) -> Option<Vec<u32>> {
        let notes = &self.notes.state.notes;
        let (mut at, mut steps, mut found) = (0, 0u64, Vec::new());
        while at < notes.len() {
            steps += 1;
            if limit.is_some_and(|limit| steps > limit) {
                return None;
            }
            if notes[at]
                .text
                .split_whitespace()
                .any(|w| w.eq_ignore_ascii_case(word))
            {
                found.push(notes[at].id);
                at += 1;
            }
        }
        Some(found)
    }
}

impl Component for Bad {
    const SCHEMA: u32 = comp_notes::SCHEMA;

    fn init() -> Self {
        #[cfg(target_family = "wasm")]
        if BUG == Bug::Import {
            // SAFETY: the host refuses to link a module importing this, so it never runs.
            unsafe { teleport() }
        }
        Self::with(Notes::init(), BUG)
    }

    fn import(bytes: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        Notes::import(bytes, from_schema).map(|notes| Self::with(notes, BUG))
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        self.notes.export(schema)
    }

    fn handle(&mut self, request: &str) -> String {
        let (command, rest) = request.split_once(' ').unwrap_or((request, ""));
        match (command, self.bug) {
            ("find", Bug::Spin) if !rest.trim().is_empty() => {
                let ids = self.find_spinning(rest.trim(), None).unwrap_or_default();
                match ids.is_empty() {
                    true => String::from("ok none"),
                    false => {
                        let ids: Vec<String> = ids.iter().map(u32::to_string).collect();
                        format!("ok {}", ids.join(" "))
                    }
                }
            }
            ("add", Bug::Trap) => {
                let reply = self.notes.handle(request);
                let count = self.notes.state.notes.len();
                if let Some(id) = reply.strip_prefix("ok ").and_then(|id| id.parse().ok()) {
                    // The bug: one slot per note, as if there were never more than six.
                    self.recent[count - 1] = id;
                }
                reply
            }
            _ => self.notes.handle(request),
        }
    }

    fn health(&self) -> u32 {
        self.notes.health()
    }

    fn self_test(&self) -> u32 {
        self.notes.self_test()
    }
}

compsdk::export!(Bad);

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(texts: &[&str]) -> Notes {
        let mut notes = Notes::init();
        for text in texts {
            notes.add(text);
        }
        notes
    }

    #[test]
    fn the_health_bug_forgets_word_counts() {
        let bad = Bad::with(notes(&["buy milk"]), Bug::Health);
        assert_eq!(bad.health(), comp_notes::WORD_COUNTS_WRONG);
        assert_eq!(Bad::with(notes(&["buy milk"]), Bug::None).health(), 0);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn the_trap_bug_overruns_on_the_seventh_note() {
        let mut bad = Bad::with(notes(&["a", "b", "c", "d", "e"]), Bug::Trap);
        assert_eq!(bad.handle("add f"), "ok 6");
        bad.handle("add g");
    }

    #[test]
    fn the_spin_bug_loops_when_the_first_note_does_not_match() {
        let bad = Bad::with(notes(&["fix the roof", "walk"]), Bug::Spin);
        assert_eq!(bad.find_spinning("fix", Some(1000)), None);
        let only = Bad::with(notes(&["fix the roof"]), Bug::Spin);
        assert_eq!(only.find_spinning("roof", Some(1000)), Some(alloc::vec![1]));
    }
}
