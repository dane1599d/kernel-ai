//! Handles between two tables: out of the sender's ([`super::take_for`]) and
//! not yet in the receiver's ([`super::give`]), while a call or a reply that
//! carries them waits. A sweep that frees what no table names would free the
//! object under them, so each one is counted as in transit for as long as a
//! [`Moving`] holds it, and the sweeps count that as named (audit K #1).
//!
//! Only kinds whose objects a sweep frees are counted: `NetDest` records
//! ([`crate::net::dest::transit`]).

use alloc::vec::Vec;
use captable::Entry;
use kabi::handle::Kind;

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
        Self { entries }
    }
}

impl Drop for Moving {
    fn drop(&mut self) {
        for entry in &self.entries {
            count(entry, false);
        }
    }
}

fn count(entry: &Entry, up: bool) {
    if entry.kind == Kind::NetDest {
        crate::net::dest::transit(entry.object, up);
    }
}
