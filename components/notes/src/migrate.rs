//! From schema 1 to schema 2: each text keeps its position as its id, and gets its
//! word count.

use crate::state::{Note, V1, V2, words};

pub fn from_v1(old: V1) -> V2 {
    let notes: alloc::vec::Vec<Note> = old
        .texts
        .into_iter()
        .zip(1..)
        .map(|(text, id)| Note {
            id,
            words: words(&text),
            text,
        })
        .collect();
    let next_id = notes.len() as u32 + 1;
    V2 { notes, next_id }
}
