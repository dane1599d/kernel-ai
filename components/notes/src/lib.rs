//! `notes`, the live component: short notes with ids, state schema 2.
//!
//! Requests: `add <text>` (replies with the new note's id), `count` (notes and
//! words), `get <id>`, `find <word>` (the ids of notes with that word) and `about`.
//! It reads schema 1, written by version 1, and migrates it (see `migrate`).

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

pub mod migrate;
pub mod state;
#[cfg(test)]
mod tests;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use compsdk::{Component, ImportError};
use state::{Note, V1, V2, words};

pub const SCHEMA: u32 = 2;
pub const VERSION: &str = match option_env!("KAI_VERSION") {
    Some(version) => version,
    None => "dev",
};

/// Health check results.
pub const WORD_COUNTS_WRONG: u32 = 1;
pub const IDS_OUT_OF_ORDER: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notes {
    pub state: V2,
}

impl Notes {
    pub fn add(&mut self, text: &str) -> u32 {
        let id = self.state.next_id;
        self.state.next_id += 1;
        self.state.notes.push(Note {
            id,
            words: words(text),
            text: text.to_string(),
        });
        id
    }

    /// Whether a note with `text` still fits in the state the host can keep.
    pub fn has_room_for(&self, text: &str) -> bool {
        // The note's id, word count and framing take well under 24 bytes.
        let size = compsdk::state::encode(SCHEMA, &self.state).len() + text.len() + 24;
        size <= compsdk::MAX_STATE
    }

    pub fn count(&self) -> String {
        let words: u32 = self.state.notes.iter().map(|note| note.words).sum();
        format!("ok {} notes, {words} words", self.state.notes.len())
    }

    /// The ids of the notes that have `word` in them, ignoring case.
    pub fn find(&self, word: &str) -> Vec<u32> {
        self.state
            .notes
            .iter()
            .filter(|note| {
                note.text
                    .split_whitespace()
                    .any(|w| w.eq_ignore_ascii_case(word))
            })
            .map(|note| note.id)
            .collect()
    }

    pub fn get(&self, id: &str) -> String {
        id.parse::<u32>()
            .ok()
            .and_then(|id| self.state.notes.iter().find(|note| note.id == id))
            .map_or_else(
                || String::from("err no such note"),
                |note| format!("ok {}", note.text),
            )
    }
}

impl Component for Notes {
    const SCHEMA: u32 = SCHEMA;

    fn init() -> Self {
        Self {
            state: V2 {
                notes: Vec::new(),
                next_id: 1,
            },
        }
    }

    fn import(bytes: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        let state = match from_schema {
            SCHEMA => compsdk::state::decode::<V2>(bytes, SCHEMA),
            1 => compsdk::state::decode::<V1>(bytes, 1).map(migrate::from_v1),
            _ => return Err(ImportError::Unsupported),
        };
        state.map(|state| Self { state }).ok_or(ImportError::Bad)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| compsdk::state::encode(SCHEMA, &self.state))
    }

    fn handle(&mut self, request: &str) -> String {
        let (command, rest) = request.split_once(' ').unwrap_or((request, ""));
        let rest = rest.trim();
        match command {
            "add" if !rest.is_empty() => {
                if !self.has_room_for(rest) {
                    return String::from("err notes are full");
                }
                format!("ok {}", self.add(rest))
            }
            "count" => self.count(),
            "get" => self.get(rest),
            "find" if !rest.is_empty() => {
                let ids = self.find(rest);
                if ids.is_empty() {
                    return String::from("ok none");
                }
                let mut reply = String::from("ok");
                for id in ids {
                    let id = id.to_string();
                    // Keep the reply within what one reply may hold.
                    if reply.len() + id.len() + 5 > compsdk::MAX_REPLY {
                        reply.push_str(" ...");
                        break;
                    }
                    reply.push(' ');
                    reply.push_str(&id);
                }
                reply
            }
            "about" => format!("ok notes {VERSION} schema {SCHEMA}"),
            _ => format!("err unknown request {command:?}"),
        }
    }

    fn health(&self) -> u32 {
        if self
            .state
            .notes
            .iter()
            .any(|note| note.words != words(&note.text))
        {
            return WORD_COUNTS_WRONG;
        }
        let mut last = 0;
        for note in &self.state.notes {
            if note.id <= last || note.id >= self.state.next_id {
                return IDS_OUT_OF_ORDER;
            }
            last = note.id;
        }
        0
    }

    fn self_test(&self) -> u32 {
        // Floating point, which runs as software floating point in ring 3.
        let sample = [1.5f64, 2.5, 3.5];
        let mean = sample.iter().sum::<f64>() / sample.len() as f64;
        u32::from((mean * 4.0) as u64 != 10)
    }
}

#[cfg(feature = "exports")]
compsdk::export!(Notes);
