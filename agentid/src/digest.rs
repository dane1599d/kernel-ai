//! The digest of a question: which call a yes answered.
//!
//! SHA-256 over a tag, the log entry, the row and the question exactly as the person
//! was shown it, each length-prefixed so no row and text can be shifted into another
//! pair. It is the same idea as the `ONCE` handle's digest (ADR 0001 D6: "what was
//! shown is what runs, once"), kept here so the roster can bind a yes to the call it
//! answered before any handle exists.

use sha2::{Digest as _, Sha256};

const TAG: &[u8] = b"kai-ask-digest 1";

/// A question's digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Digest([u8; 32]);

impl Digest {
    pub(crate) fn of(entry: u64, row: &str, shown: &str) -> Self {
        let mut hash = Sha256::new();
        hash.update(TAG);
        hash.update(entry.to_le_bytes());
        for text in [row, shown] {
            hash.update((text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
        }
        let mut out = [0u8; 32];
        for (to, from) in out.iter_mut().zip(hash.finalize().iter()) {
            *to = *from;
        }
        Self(out)
    }
}
