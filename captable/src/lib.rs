//! One process's handle table (ADR 0001 D1), kept free of kernel code so every
//! rule can be tested on the host; the kernel wires it in `kernel/src/handle/`.
//!
//! A handle names an entry: a kind, rights, flags, the object and a badge. The
//! rules are Zircon's: a duplicate asks for a subset of the source's rights and
//! needs DUPLICATE on it; a replace swaps a handle for a narrower one and closes
//! the old one either way; a close always works and frees the slot; a `ONCE`
//! handle is closed by its first use. Rights only ever shrink.
//!
//! Each slot keeps a generation, bumped when its entry closes, that is part of the
//! handle, so a closed handle never resolves again, even after its slot is reused.
//! A slot whose generation would wrap is retired, never reused. The table starts
//! with [`TABLE_START`] slots and grows on demand to [`TABLE_MAX`].

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use kabi::handle::{Flags, Handle, INDEX_BITS, Kind, Rights, TABLE_MAX, TABLE_START};

/// The highest generation a slot reaches before it is retired.
const MAX_GENERATION: u32 = (1 << (32 - INDEX_BITS)) - 1;

/// What a handle names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub rights: Rights,
    pub flags: Flags,
    /// The object, numbered by its kind's owner.
    pub object: u64,
    /// Read by the object's server; the kernel never interprets it.
    pub badge: u64,
}

impl Entry {
    pub const fn new(kind: Kind, rights: Rights, object: u64) -> Entry {
        Entry {
            kind,
            rights,
            flags: Flags::empty(),
            object,
            badge: 0,
        }
    }
}

/// Why a table operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Forged, stale or of the wrong kind.
    BadHandle,
    /// The handle lacks a right the operation needs.
    Permission,
    /// Asked for rights the source does not have.
    Invalid,
    /// The table is full, at [`TABLE_MAX`] or out of memory.
    Full,
}

impl From<Error> for kabi::Error {
    fn from(error: Error) -> kabi::Error {
        match error {
            Error::BadHandle => kabi::Error::BadHandle,
            Error::Permission => kabi::Error::Permission,
            Error::Invalid => kabi::Error::Invalid,
            Error::Full => kabi::Error::NoMemory,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    generation: u32,
    entry: Option<Entry>,
}

#[derive(Debug, Default)]
pub struct Table {
    slots: Vec<Slot>,
    /// Indexes of empty slots that can be reused, used last-in first-out.
    free: Vec<u16>,
}

impl Table {
    /// An empty table with room for [`TABLE_START`] entries.
    pub fn new() -> Result<Table, Error> {
        let mut table = Table::default();
        table.grow(TABLE_START)?;
        Ok(table)
    }

    /// Open entries.
    pub fn len(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.entry.is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Slots in the table, open or not.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    fn grow(&mut self, to: usize) -> Result<(), Error> {
        let to = to.min(TABLE_MAX);
        let more = to.saturating_sub(self.slots.len());
        if more == 0 {
            return Err(Error::Full);
        }
        self.slots
            .try_reserve_exact(more)
            .map_err(|_| Error::Full)?;
        self.free.try_reserve_exact(more).map_err(|_| Error::Full)?;
        let start = self.slots.len();
        self.slots.extend((0..more).map(|_| Slot {
            generation: 1,
            entry: None,
        }));
        // Lower indexes come out first.
        self.free
            .extend((start..to).rev().map(|index| index as u16));
        Ok(())
    }

    /// Stores `entry` and returns its handle.
    pub fn insert(&mut self, entry: Entry) -> Result<Handle, Error> {
        if self.free.is_empty() {
            self.grow(self.slots.len() * 2)?;
        }
        let index = usize::from(self.free.pop().ok_or(Error::Full)?);
        let slot = &mut self.slots[index];
        slot.entry = Some(entry);
        Ok(Handle::new(index, slot.generation))
    }

    /// The entry `handle` names, if it is open.
    pub fn get(&self, handle: Handle) -> Result<&Entry, Error> {
        let slot = self.slots.get(handle.index()).ok_or(Error::BadHandle)?;
        match &slot.entry {
            Some(entry) if handle.generation() == slot.generation => Ok(entry),
            _ => Err(Error::BadHandle),
        }
    }

    /// The entry `handle` names, which must be of `kind` (BadHandle otherwise)
    /// and carry every right in `rights` (Permission otherwise). A `ONCE` handle
    /// is closed by this use.
    pub fn check(&mut self, handle: Handle, kind: Kind, rights: Rights) -> Result<Entry, Error> {
        let entry = *self.get(handle)?;
        if entry.kind != kind {
            return Err(Error::BadHandle);
        }
        if !entry.rights.contains(rights) {
            return Err(Error::Permission);
        }
        if entry.flags.contains(Flags::ONCE) {
            self.close(handle)?;
        }
        Ok(entry)
    }

    /// An open handle of `kind` that carries every right in `rights`, the first
    /// by index; `ONCE` handles are never picked this way.
    pub fn find(&self, kind: Kind, rights: Rights) -> Option<Handle> {
        self.iter().find_map(|(handle, entry)| {
            let fits = entry.kind == kind
                && entry.rights.contains(rights)
                && !entry.flags.contains(Flags::ONCE);
            fits.then_some(handle)
        })
    }

    /// Whether some open handle of `kind` carries every right in `rights`.
    pub fn holds(&self, kind: Kind, rights: Rights) -> bool {
        self.find(kind, rights).is_some()
    }

    /// A second handle to `handle`'s object with `rights`, a subset of its own.
    pub fn dup(&mut self, handle: Handle, rights: Rights) -> Result<Handle, Error> {
        let entry = *self.get(handle)?;
        if !entry.rights.contains(Rights::DUPLICATE) {
            return Err(Error::Permission);
        }
        if !entry.rights.contains(rights) {
            return Err(Error::Invalid);
        }
        self.insert(Entry { rights, ..entry })
    }

    /// Swaps `handle` for one with `rights`, a subset of its own. The old handle
    /// is closed even when the new rights are refused.
    pub fn replace(&mut self, handle: Handle, rights: Rights) -> Result<Handle, Error> {
        let entry = self.close(handle)?;
        if !entry.rights.contains(rights) {
            return Err(Error::Invalid);
        }
        self.insert(Entry { rights, ..entry })
    }

    /// Closes `handle`; its entry, as it was.
    pub fn close(&mut self, handle: Handle) -> Result<Entry, Error> {
        self.get(handle)?;
        let index = handle.index();
        let slot = &mut self.slots[index];
        let entry = slot.entry.take().ok_or(Error::BadHandle)?;
        if slot.generation < MAX_GENERATION {
            slot.generation += 1;
            self.free.push(index as u16);
        }
        Ok(entry)
    }

    /// Every open handle and its entry, by index.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &Entry)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            let entry = slot.entry.as_ref()?;
            Some((Handle::new(index, slot.generation), entry))
        })
    }
}

#[cfg(test)]
mod tests;
