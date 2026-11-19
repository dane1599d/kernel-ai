//! ADR 0013's two volumes (agents-os.md M1 and S1), unreserved after the v0.1
//! tag:
//!
//! - **`memory`** (4): the user's approved memories. Its undo snapshots live one
//!   hour ([`MEMORY_UNDO_LIFE`]), not ADR 0004's 30 days, and [`Fs::purge`]
//!   drops every snapshot of it that still holds a record, so a forgotten
//!   record is gone from every tree once its 10-minute undo has passed.
//! - **`index`** (5): the search index, derived data that is rebuilt rather
//!   than restored. It is never backed up: no action takes an undo snapshot of
//!   it, and nothing else snapshots it.
//!
//! `mkfs` still makes only system, home and models, so the format's golden
//! images are unchanged: [`Fs::add_agent_volumes`] adds the two to any disk
//! that lacks them, a new one or one an older slot wrote, and the `fs`
//! service calls it at start. No app route reaches either (fsserve mints
//! badges in system, home and models only).

use crate::Error;
use crate::dev::BlockDev;
use crate::fs::{Fs, Vol};
use crate::items::{FileType, Inode, ROOT};
use crate::undo::View;
use crate::vol::{SnapKind, UNDO_LIFE};
use alloc::vec::Vec;

pub const MEMORY: u64 = 4;
pub const INDEX: u64 = 5;
/// The volumes [`Fs::add_agent_volumes`] makes, with their names.
pub const AGENT_VOLUMES: [(u64, &str); 2] = [(MEMORY, "memory"), (INDEX, "index")];
/// How long an undo snapshot of `memory` lives: one hour, in nanoseconds.
pub const MEMORY_UNDO_LIFE: u64 = 3600 * 1_000_000_000;

/// How long an action's undo snapshot of `volume` lives; `None` for one that
/// takes none (`index`).
pub fn undo_life(volume: u64) -> Option<u64> {
    match volume {
        MEMORY => Some(MEMORY_UNDO_LIFE),
        INDEX => None,
        _ => Some(UNDO_LIFE),
    }
}

impl<D: BlockDev> Fs<D> {
    /// Adds `memory` and `index`, each with an empty root folder, where the
    /// disk lacks them; how many it added. Not committed: the caller commits.
    /// Never inside an action, and refused on a read-only mount.
    pub fn add_agent_volumes(&mut self) -> Result<usize, Error> {
        if !self.writable() {
            return Err(Error::ReadOnly);
        }
        if self.action.is_some() {
            return Err(Error::Invalid("agent volumes are added outside an action"));
        }
        let had = self.volumes();
        let active = self.volume();
        let mut added = 0;
        for (id, name) in AGENT_VOLUMES {
            if had.iter().any(|(v, n)| *v == id || n == name) {
                continue;
            }
            self.vols.insert(id, Vol::new(name));
            self.set_volume(id)?;
            let root = self.alloc_oid()?;
            debug_assert_eq!(root, ROOT);
            let inode = Inode::new(FileType::Dir, ROOT, self.clock);
            self.put_inode(ROOT, &inode)?;
            added += 1;
        }
        self.set_volume(active)?;
        Ok(added)
    }

    /// Drops every snapshot of the `memory` volume whose tree still holds
    /// object `ino`, whatever its kind: the purge after a record is forgotten
    /// (ADR 0013 §3). How many it dropped. Not committed; the blocks they
    /// alone held go to the next GC pass.
    pub fn purge(&mut self, ino: u64) -> Result<usize, Error> {
        if !self.writable() {
            return Err(Error::ReadOnly);
        }
        let active = self.volume();
        self.set_volume(MEMORY)?;
        let snaps: Vec<_> = self
            .snapshots()
            .iter()
            .map(|(id, s)| (*id, s.root))
            .collect();
        let mut dropped = 0;
        let result = (|| {
            for (id, root) in snaps {
                if self.inode_in(View::Tree(root), ino)?.is_some() {
                    self.drop_snapshot(id)?;
                    dropped += 1;
                }
            }
            Ok(dropped)
        })();
        self.set_volume(active)?;
        result
    }

    /// Whether an undo snapshot has expired by `now`: what tells the `fs`
    /// service to run [`Fs::expire`] while space is not low, so `memory`'s
    /// hour is kept whatever the free space.
    pub fn expiry_due(&self, now: u64) -> bool {
        self.vols.values().any(|vol| {
            vol.snaps
                .values()
                .any(|s| s.kind == SnapKind::Undo && s.expiry != 0 && s.expiry <= now)
        })
    }
}
