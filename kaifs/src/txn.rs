//! Actions (ADR 0004 §4; ADR 0002; P2 task 6): one transaction an action,
//! begun by [`Fs::begin`] and ended by its commit or by [`Fs::abort`].
//!
//! - While an action is open it counts as a snapshot of the generation it began
//!   after, so no block its undo snapshot will hold is freed under it.
//! - Its commit writes, for each volume it changed, an undo snapshot of the
//!   volume's root before it (kind undo, expiring after 30 days, pinned by the
//!   action) and its UNDO record: the generation before, the action, the time,
//!   the snapshot and every object it changed. Both land in the same commit as
//!   the changes, so after a crash an action is wholly done, with its undo, or
//!   not done at all.
//! - [`Fs::abort`] reads the last commit again, in place: nothing an open action
//!   wrote is reachable from it.

use crate::Error;
use crate::dev::BlockDev;
use crate::fs::{Fs, Snap};
use crate::io::Meta;
use crate::tree::Key;
use crate::vol::{
    K_UNDO, SnapKind, UNDO_LIFE, Undo, decode_undo, encode_snapshot, encode_undo, snap_key,
    undo_key,
};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// An open action (§4; ADR 0002): its id, and the generation it began after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub id: u64,
    pub base: u64,
}

impl<D: BlockDev> Fs<D> {
    /// The UNDO record of the action committed as `txn` in the active volume.
    pub fn undo_record(&mut self, txn: u64) -> Result<Option<Undo>, Error> {
        let records = self.undo_scan(undo_key(self.active, txn, 0), Some(txn))?;
        Ok(records.into_iter().next())
    }

    /// Every UNDO record of the active volume, oldest first.
    pub fn undo_records(&mut self) -> Result<Vec<Undo>, Error> {
        self.undo_scan(undo_key(self.active, 0, 0), None)
    }

    fn undo_scan(&mut self, from: Key, only: Option<u64>) -> Result<Vec<Undo>, Error> {
        let active = self.active;
        let mut parts: BTreeMap<u64, Vec<Vec<u8>>> = BTreeMap::new();
        let mut gap = false;
        self.volumes
            .scan(&from, &mut Meta(&mut self.io), &mut |key, value| {
                let (txn, part) = (key.off >> 16, key.off & 0xFFFF);
                let mine = key.oid == active && key.kind == K_UNDO && only.is_none_or(|t| t == txn);
                if mine {
                    let list = parts.entry(txn).or_default();
                    gap |= list.len() as u64 != part;
                    list.push(value.to_vec());
                }
                mine
            })?;
        if gap {
            return Err(Error::Corrupt("undo record parts"));
        }
        parts
            .into_iter()
            .map(|(txn, list)| decode_undo(txn, &list))
            .collect()
    }

    /// Opens action `action` (§4): its changes, and for each volume it changes an
    /// undo snapshot and an UNDO record, land in its one commit. Changes made
    /// outside an action must be committed first.
    pub fn begin(&mut self, action: u64) -> Result<(), Error> {
        if !self.writable() {
            return Err(Error::ReadOnly);
        }
        if self.action.is_some() {
            return Err(Error::Invalid("an action is already open"));
        }
        if !self.is_clean() {
            return Err(Error::Invalid(
                "changes outside an action: commit them first",
            ));
        }
        self.action = Some(Action {
            id: action,
            base: self.committed,
        });
        Ok(())
    }

    /// The open action, if any.
    pub fn action(&self) -> Option<Action> {
        self.action
    }

    /// Drops everything since the last commit, an open action and a running GC
    /// pass with it: the allocator and the volumes are read again from the newest
    /// commit on the disk, as a mount reads them. The `Fs` stays where it is, so a
    /// server holding it needs nothing moved; after an I/O error mid-operation,
    /// this is how the transaction is dropped. The volume calls act on is kept.
    ///
    /// Returns the generation it read. That is the last commit, or the one an
    /// error interrupted if that one had landed whole (an error after a
    /// superblock slot was written): then its changes are on the disk, and the
    /// caller must say so, not that it failed.
    pub fn abort(&mut self) -> Result<u64, Error> {
        let (sb, _) = Self::newest(&mut self.io.dev)?;
        if sb.generation < self.committed {
            return Err(Error::Corrupt("the disk lost the last commit"));
        }
        let loaded = Self::load(&mut self.io.dev, &sb, self.faults)?;
        self.io.alloc = loaded.alloc;
        self.io.txn = sb.generation + 1;
        self.io.gc_pin = 0;
        self.io.deferred.clear();
        self.volumes = loaded.volumes;
        self.vols = loaded.vols;
        self.action = None;
        self.gc = None;
        self.committed = sb.generation;
        if !self.vols.contains_key(&self.active) {
            self.active = loaded.active;
        }
        Ok(sb.generation)
    }

    /// For each volume the open action changed: an undo snapshot of its root
    /// before the action, and the action's UNDO record. Everything is encoded
    /// before anything is written.
    pub(crate) fn write_undo(&mut self, action: Action, txn: u64) -> Result<(), Error> {
        let mut writes: Vec<(Key, Vec<u8>)> = Vec::new();
        let mut snaps: Vec<(u64, u64, Snap)> = Vec::new();
        for (&id, vol) in &self.vols {
            if !vol.live_changed && vol.touched.is_empty() {
                continue;
            }
            let snap_id = vol.next_snap;
            let snap = Snap {
                root: vol.base.0,
                generation: action.base,
                next_oid: vol.base.1,
                kind: SnapKind::Undo,
                expiry: self.clock.saturating_add(UNDO_LIFE),
                pin: action.id,
            };
            let undo = Undo {
                txn,
                before: action.base,
                action: action.id,
                time: self.clock,
                snapshot: snap_id,
                objects: vol.touched.iter().copied().collect(),
            };
            for (part, value) in encode_undo(&undo)?.into_iter().enumerate() {
                writes.push((undo_key(id, txn, part as u64), value));
            }
            writes.push((snap_key(id, snap_id), encode_snapshot(&snap)));
            snaps.push((id, snap_id, snap));
        }
        for (key, value) in writes {
            self.volumes.insert(key, value, &mut Meta(&mut self.io))?;
        }
        for (id, snap_id, snap) in snaps {
            if let Some(vol) = self.vols.get_mut(&id) {
                vol.snaps.insert(snap_id, snap);
                vol.next_snap = snap_id + 1;
                vol.touched.clear();
            }
        }
        Ok(())
    }
}

/// How many parts `undo`'s record has.
pub(crate) fn undo_parts(undo: &Undo) -> u64 {
    encode_undo(undo).map_or(1, |parts| parts.len() as u64)
}
