//! The state schemas `notes` reads and writes.

use alloc::string::String;
use alloc::vec::Vec;
use minicbor::{Decode, Encode};

/// Schema 1, written by version 1: the texts, numbered by position.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V1 {
    #[n(0)]
    pub texts: Vec<String>,
}

/// Schema 2: notes with stable ids and their word counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V2 {
    #[n(0)]
    pub notes: Vec<Note>,
    /// The id the next note gets.
    #[n(1)]
    pub next_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Note {
    #[n(0)]
    pub id: u32,
    #[n(1)]
    pub text: String,
    #[n(2)]
    pub words: u32,
}

pub fn words(text: &str) -> u32 {
    text.split_whitespace().count() as u32
}
