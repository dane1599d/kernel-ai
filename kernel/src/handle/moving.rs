//! Handles between two tables: out of the sender's ([`super::take_for`]) and
//! not yet in the receiver's ([`super::give`]), while a call or a reply that
//! carries them waits. A sweep that frees what no table names would free the
//! object under them, so each one is counted as in transit for as long as a
//! [`Moving`] holds it, and the sweeps count that as named (audit K #1).
//!
//! Only kinds whose objects a sweep frees are counted: `NetDest` records
//! ([`crate::net::dest::transit`]).
//!
//! A sweep reads a record's transit count and then the tables, and a handle
//! may move between the two: out of a table after the count was read as 0,
//! before that table is scanned. Every move moves [`moves`] on, under the
//! source table's lock for a take, so a sweep that sees it change while it
//! looks keeps the record (#1150's review).

use alloc::vec::Vec;
use captable::Entry;
use core::sync::atomic::{AtomicU64, Ordering};
use kabi::handle::Kind;

/// Moves on as entries start or end a move.
static MOVES: AtomicU64 = AtomicU64::new(0);

/// How many times entries have started or ended a move: a sweep reads it
/// before and after it looks at one object, and keeps the object if it
/// changed.
pub fn moves() -> u64 {
    MOVES.load(Ordering::SeqCst)
}

/// Entries on their way to a table; counted as in transit until dropped.
#[derive(Debug, Default)]
pub struct Moving {
    entries: Vec<Entry>,
}

impl Moving {
    pub const fn none() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The entries, still counted until this is dropped.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

impl From<Vec<Entry>> for Moving {
    fn from(entries: Vec<Entry>) -> Self {
        for entry in &entries {
            count(entry, true);
        }
        MOVES.fetch_add(1, Ordering::SeqCst);
        Self { entries }
    }
}

impl Drop for Moving {
    fn drop(&mut self) {
        for entry in &self.entries {
            count(entry, false);
        }
        MOVES.fetch_add(1, Ordering::SeqCst);
    }
}

fn count(entry: &Entry, up: bool) {
    if entry.kind == Kind::NetDest {
        crate::net::dest::transit(entry.object, up);
    }
}
