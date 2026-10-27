//! `notes`, the first app: short notes with ids, state schema 2. Its actions are
//! `app:notes/actions@1.0.0` (`wit/notes.wit`): `add` (the new note's id),
//! `count` (notes and words), `find` (the ids of notes with a word), `get` and
//! `about`. It reads schema 1, written by version 1, and migrates it.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

pub mod migrate;
pub mod state;
#[cfg(test)]
mod tests;
pub mod ui;

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
/// Most ids `find` returns, so its reply fits one message.
pub const MAX_FOUND: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notes {
    pub state: V2,
}

impl Actions for Notes {
    fn add(&mut self, text: &str) -> Result<u32, String> {
        if !self.has_room_for(text) {
            return Err(String::from("notes are full"));
        }
        let id = self.state.next_id;
        self.state.next_id += 1;
        self.state.notes.push(Note {
            id,
            words: words(text),
            text: text.to_string(),
        });
        Ok(id)
    }

    fn count(&self) -> Tally {
        let words: u32 = self.state.notes.iter().map(|note| note.words).sum();
        Tally {
            notes: u32::try_from(self.state.notes.len()).unwrap_or(u32::MAX),
            words,
        }
    }

    /// The ids of the notes that have `word` in them, ignoring case.
    fn find(&self, word: &str) -> Vec<u32> {
        self.state
            .notes
            .iter()
            .filter(|note| {
                note.text
                    .split_whitespace()
                    .any(|w| w.eq_ignore_ascii_case(word))
            })
            .map(|note| note.id)
            .take(MAX_FOUND)
            .collect()
    }

    fn get(&self, id: u32) -> Option<String> {
        self.state
            .notes
            .iter()
            .find(|note| note.id == id)
            .map(|note| note.text.clone())
    }

    fn about(&self) -> String {
        format!("notes {VERSION} schema {SCHEMA}")
    }
}

impl Notes {
    /// Whether a note with `text` still fits in the state the host can keep.
    pub fn has_room_for(&self, text: &str) -> bool {
        // The note's id, word count and framing take well under 24 bytes.
        let size = compsdk::state::encode(SCHEMA, &self.state).len() + text.len() + 24;
        size <= compsdk::MAX_STATE
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
        // Floating point, which runs as hardware floating point in comphost.
        let sample = [1.5f64, 2.5, 3.5];
        let mean = sample.iter().sum::<f64>() / sample.len() as f64;
        u32::from((mean * 4.0) as u64 != 10)
    }
}

/// How many notes there are, and how many words they hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tally {
    pub notes: u32,
    pub words: u32,
}

/// `app:notes/actions@1.0.0`, which every notes version implements.
pub trait Actions {
    /// Adds a note; its id, or why not.
    fn add(&mut self, text: &str) -> Result<u32, String>;
    fn count(&self) -> Tally;
    /// The ids of the notes that have `word`, at most [`MAX_FOUND`].
    fn find(&self, word: &str) -> Vec<u32>;
    fn get(&self, id: u32) -> Option<String>;
    fn about(&self) -> String;
}

/// `app:notes/actions@1.0.0`'s bindings (`wit/notes.wit`, world `actions-only`).
#[cfg(target_family = "wasm")]
#[doc(hidden)]
pub mod bind {
    wit_bindgen::generate!({
        world: "app:notes/actions-only",
        path: ["../../wit/kai-live", "../../wit/kai-app", "wit"],
        pub_export_macro: true,
        export_macro_name: "export_actions",
        default_bindings_module: "comp_notes::bind",
    });
}

#[doc(hidden)]
pub mod __private {
    pub use alloc::string::String;
    pub use alloc::vec::Vec;
    pub use compsdk;
}

/// Binds a notes version: `kai:live/lifecycle` through compsdk, and
/// `app:notes/actions@1.0.0` to its [`Actions`].
#[macro_export]
macro_rules! app {
    ($app:ty) => {
        $crate::__private::compsdk::component!($app);

        #[cfg(target_family = "wasm")]
        const _: () = {
            use $crate::__private::{String, Vec};
            use $crate::Actions;
            use $crate::bind::exports::app::notes::actions::{Guest, Tally};

            struct Bound;

            fn started<R>(f: impl FnOnce(&mut $app) -> R) -> R {
                match $crate::__private::compsdk::with::<$app, R>(f) {
                    Some(result) => result,
                    None => panic!("notes was called before it started"),
                }
            }

            impl Guest for Bound {
                fn add(text: String) -> Result<u32, String> {
                    started(|notes| notes.add(&text))
                }
                fn count() -> Tally {
                    let tally = started(|notes| notes.count());
                    Tally {
                        notes: tally.notes,
                        words: tally.words,
                    }
                }
                fn find(word: String) -> Vec<u32> {
                    started(|notes| notes.find(&word))
                }
                fn get(id: u32) -> Option<String> {
                    started(|notes| notes.get(id))
                }
                fn about() -> String {
                    started(|notes| notes.about())
                }
            }

    $crate::bind::export_actions!(Bound with_types_in $crate::bind);
        };
    };
}

#[cfg(feature = "exports")]
app!(Notes);
