//! `notes` version 1: a numbered list of short texts, state schema 1.
//!
//! Requests: `add <text>` (replies with the count), `count`, `get <n>` (from 1) and
//! `about`. It reads only schema 1, so it cannot run on a newer state.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
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

    fn handle(&mut self, request: &str) -> String {
        let (command, rest) = request.split_once(' ').unwrap_or((request, ""));
        match command {
            "add" if !rest.trim().is_empty() => {
                self.texts.push(rest.trim().to_string());
                format!("ok {}", self.texts.len())
            }
            "count" => format!("ok {}", self.texts.len()),
            "get" => match rest.trim().parse::<usize>() {
                Ok(n) if (1..=self.texts.len()).contains(&n) => format!("ok {}", self.texts[n - 1]),
                _ => String::from("err no such note"),
            },
            "about" => format!("ok notes {VERSION} schema {SCHEMA}"),
            _ => format!("err unknown request {command:?}"),
        }
    }

    fn health(&self) -> u32 {
        u32::from(self.texts.iter().any(|text| text.is_empty()))
    }

    fn self_test(&self) -> u32 {
        0
    }
}

compsdk::export!(Notes);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_added_counted_and_read_back() {
        let mut notes = Notes::init();
        assert_eq!(notes.handle("add buy milk"), "ok 1");
        assert_eq!(notes.handle("add  call bob "), "ok 2");
        assert_eq!(notes.handle("count"), "ok 2");
        assert_eq!(notes.handle("get 2"), "ok call bob");
        assert_eq!(notes.handle("get 3"), "err no such note");
        assert_eq!(notes.handle("add"), "err unknown request \"add\"");
        assert_eq!(notes.health(), 0);
    }

    #[test]
    fn it_reads_and_writes_only_schema_1() {
        let mut notes = Notes::init();
        notes.handle("add a");
        let saved = notes.export(SCHEMA).unwrap();
        assert_eq!(Notes::import(&saved, 1), Ok(notes.clone()));
        assert_eq!(Notes::import(&saved, 2), Err(ImportError::Unsupported));
        assert_eq!(Notes::import(b"junk", 1), Err(ImportError::Bad));
        assert_eq!(notes.export(2), None);
    }
}
