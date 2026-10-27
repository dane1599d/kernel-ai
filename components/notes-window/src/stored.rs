//! Where Notes' window build keeps its notes (P4 task 6): in its own
//! `kai:app` storage, one key per note and an index, so the checkpoint holds
//! almost nothing. `comp-notes` keeps its logic and its in-memory [`V2`]; this
//! module only moves notes between that and storage. When `fs` backs storage
//! (a folder per app, ADR 0004 section 5), these keys become that folder's
//! files and nothing here changes.
//!
//! Storage is written only inside action calls. Notes that came another way, a
//! schema 1 or 2 state migrated in, or an edit from the window between calls,
//! are pending: the checkpoint carries them (schema 3's `pending`) until the
//! next `add` writes them out.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_notes::state::{Note, V2, words};
use compsdk::app;
use minicbor::{Decode, Encode};

/// The window build's state schema.
pub const SCHEMA: u32 = 3;
/// The index's key.
pub const INDEX: &str = "notes/index";

/// The key of note `id`.
pub fn key(id: u32) -> String {
    format!("notes/{id}")
}

/// Which notes are in storage, in order, and the next id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct Index {
    #[n(0)]
    pub ids: Vec<u32>,
    #[n(1)]
    pub next_id: u32,
}

/// Schema 3: the notes not yet in storage, if any.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V3 {
    #[n(0)]
    pub pending: Option<V2>,
}

/// The notes in storage, as `comp-notes` holds them; empty when there are none.
pub fn load() -> V2 {
    let index: Index = app::get(INDEX)
        .and_then(|bytes| minicbor::decode(&bytes).ok())
        .unwrap_or_default();
    let notes = index
        .ids
        .iter()
        .filter_map(|&id| {
            let text = String::from_utf8(app::get(&key(id))?).ok()?;
            Some(Note {
                id,
                words: words(&text),
                text,
            })
        })
        .collect();
    V2 {
        notes,
        next_id: index.next_id.max(1),
    }
}

/// Writes `state` to storage: every note that differs from what is stored, the
/// index, and the removal of notes no longer there.
pub fn save(state: &V2) {
    let old: Index = app::get(INDEX)
        .and_then(|bytes| minicbor::decode(&bytes).ok())
        .unwrap_or_default();
    for note in &state.notes {
        let stored = app::get(&key(note.id));
        if stored.as_deref() != Some(note.text.as_bytes()) {
            app::put(&key(note.id), note.text.as_bytes());
        }
    }
    for id in old
        .ids
        .iter()
        .filter(|id| !state.notes.iter().any(|n| n.id == **id))
    {
        app::delete(&key(*id));
    }
    let index = Index {
        ids: state.notes.iter().map(|n| n.id).collect(),
        next_id: state.next_id,
    };
    if index != old {
        let mut bytes = Vec::new();
        // Encoding into a Vec cannot fail.
        let _ = minicbor::encode(&index, &mut bytes);
        app::put(INDEX, &bytes);
    }
}
