//! `notes` version 1: a numbered list of short texts, state schema 1, on the same
//! `app:notes/actions@1.0.0` as version 2. Ids are positions from 1, and the word
//! count is computed from the texts, which version 2 stores instead. It reads only
//! schema 1, so it cannot run on a newer state.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use comp_notes::{Actions, MAX_FOUND, Tally};
use compsdk::{Component, ImportError, state};
use minicbor::{Decode, Encode};

pub const SCHEMA: u32 = 1;
const VERSION: &str = match option_env!("KAI_VERSION") {
    Some(version) => version,
    None => "dev",
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct Notes {
    #[n(0)]
    pub texts: Vec<String>,
}

impl Actions for Notes {
    fn add(&mut self, text: &str) -> Result<u32, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err(String::from("a note needs some text"));
        }
        self.texts.push(text.to_string());
        Ok(self.texts.len() as u32)
    }

    fn count(&self) -> Tally {
        Tally {
            notes: self.texts.len() as u32,
            words: self
                .texts
                .iter()
                .map(|t| t.split_whitespace().count() as u32)
                .sum(),
        }
    }

    fn find(&self, word: &str) -> Vec<u32> {
        (1..)
            .zip(&self.texts)
            .filter(|(_, text)| {
                text.split_whitespace()
                    .any(|w| w.eq_ignore_ascii_case(word))
            })
            .map(|(id, _)| id)
            .take(MAX_FOUND)
            .collect()
    }

    fn get(&self, id: u32) -> Option<String> {
        let index = (id as usize).checked_sub(1)?;
        self.texts.get(index).cloned()
    }

    fn about(&self) -> String {
        format!("notes {VERSION} schema {SCHEMA}")
    }
}

impl Component for Notes {
    const SCHEMA: u32 = SCHEMA;

    fn init() -> Self {
        Self::default()
    }

    fn import(bytes: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        state::decode(bytes, SCHEMA).ok_or(ImportError::Bad)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| state::encode(SCHEMA, self))
    }

    fn health(&self) -> u32 {
        u32::from(self.texts.iter().any(|text| text.is_empty()))
    }

    fn self_test(&self) -> u32 {
        0
    }
}

comp_notes::app!(Notes);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_added_counted_and_read_back() {
        let mut notes = Notes::init();
        assert_eq!(notes.add("buy milk"), Ok(1));
        assert_eq!(notes.add("  call bob "), Ok(2));
        assert_eq!(notes.count(), Tally { notes: 2, words: 4 });
        assert_eq!(notes.get(2).as_deref(), Some("call bob"));
        assert_eq!(notes.get(3), None);
        assert_eq!(notes.get(0), None);
        assert_eq!(notes.find("BOB"), [2]);
        assert!(notes.add(" ").is_err());
        assert_eq!(notes.health(), 0);
    }

    #[test]
    fn it_reads_and_writes_only_schema_1() {
        let mut notes = Notes::init();
        notes.add("a").unwrap();
        let saved = notes.export(SCHEMA).unwrap();
        assert_eq!(Notes::import(&saved, 1), Ok(notes.clone()));
        assert_eq!(Notes::import(&saved, 2), Err(ImportError::Unsupported));
        assert_eq!(Notes::import(b"junk", 1), Err(ImportError::Bad));
        assert_eq!(notes.export(2), None);
    }
}
