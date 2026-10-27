//! The engine (ADR 0004 §1-5): volumes, each a live tree and its snapshots; the
//! volume table; data records; the allocator; actions; commit and mount.
//!
//! - **Values.** A tree value is inline bytes (up to [`MAX_INLINE`]) or a pointer
//!   to a data record of up to [`MAX_RECORD`] bytes, written copy-on-write like
//!   the nodes. In a leaf a value is a tag byte, then the bytes or the record's
//!   `BlockPtr` and length.
//! - **Volumes** ([`crate::vol`]): `system`, `home` and `models`, each with its own
//!   live tree, snapshots and object id counter, in one volume table under one
//!   superblock, so one commit is atomic across all of them. Item calls act on
//!   the volume [`Fs::set_volume`] chose; `system` until then. Object ids are
//!   never reused, even across a restore, so each counter lives in the volume
//!   table, outside the trees a restore swaps.
//! - **Actions** (§4). [`Fs::begin`] opens one; its commit also writes, for each
//!   volume it changed, an undo snapshot of the volume's root before it and an
//!   UNDO record of the objects it changed. [`Fs::abort`] drops everything since
//!   the last commit.
//! - **Commit G.** Write the dirty nodes and the volume table to free blocks, then
//!   the allocator's part of its chain; FLUSH; write slot G mod 8 in both rings;
//!   FLUSH. There is no journal and no replay.
//! - **Reuse** (§3, as #88 words it). A volume block replaced in G is freed, and
//!   reusable from G+2, only if it was born after the newest snapshot of its
//!   volume that existed *when it was replaced* and is not SHARED; any other
//!   waits for the GC. The decision is made at the replacement, never at commit:
//!   spike S04-1 found that judging at commit frees blocks a restored snapshot
//!   made live again. An open action counts as a snapshot of the generation it
//!   began after, since its undo snapshot will hold that root. Volume-table and
//!   allocator blocks belong to no snapshot and are freed.
//! - **Mount** takes the newest superblock whose volume-table root and allocator
//!   head verify; once they do, any later failure is reported, never skipped.

use crate::Error;
use crate::allocator::{Allocator, run_of};
use crate::csum::CsumKind;
use crate::dev::BlockDev;
use crate::format::{self, BlockPtr, Superblock};
use crate::io::{Meta, RECORD_VALUE, TAG_RECORD, Volume};
use crate::tree::{Key, Tree};
use crate::txn::{Action, undo_parts};
use crate::vol::{
    K_SNAPSHOT, K_UNDO, K_VOLUME, SnapKind, VolEntry, decode_snapshot, decode_volume,
    encode_snapshot, encode_volume, live_key, snap_key, undo_key,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) use crate::io::inline_raw;
pub use crate::io::{
    Faults, Io, MAX_INLINE, MAX_RECORD, Value, decode_value, inline_of, record_of, value_len,
};
pub use crate::vol::{FIRST_OID, HOME, MODELS, SYSTEM, Snap, VOLUMES};

/// The volume [`Fs::format`] makes, and every call acts on until
/// [`Fs::set_volume`]: `system`.
pub const VOLUME: u64 = SYSTEM;

/// One volume in memory.
pub struct Vol {
    pub name: String,
    pub(crate) live: Tree,
    pub(crate) live_changed: bool,
    /// The live root and object id counter the last commit wrote: what an
    /// action's undo snapshot of this volume holds.
    pub(crate) base: (BlockPtr, u64),
    pub(crate) snaps: BTreeMap<u64, Snap>,
    pub(crate) next_snap: u64,
    pub(crate) next_oid: u64,
    /// The objects the open action has changed here.
    pub(crate) touched: BTreeSet<u64>,
}

impl Vol {
    fn new(name: &str) -> Self {
        Vol {
            name: name.to_string(),
            live: Tree::default(),
            live_changed: true,
            base: (BlockPtr::NULL, FIRST_OID),
            snaps: BTreeMap::new(),
            next_snap: 1,
            next_oid: FIRST_OID,
            touched: BTreeSet::new(),
        }
    }

    /// The generation of the newest snapshot it holds, 0 with none.
    pub(crate) fn newest(&self) -> u64 {
        self.snaps.values().map(|s| s.generation).max().unwrap_or(0)
    }
}

/// The volume `active` and a store over it whose reuse rule counts `pin`, an
/// open action's generation, as a snapshot.
fn split<'a, D>(
    vols: &'a mut BTreeMap<u64, Vol>,
    io: &'a mut Io<D>,
    active: u64,
    pin: u64,
) -> (&'a mut Vol, Volume<'a, D>) {
    let vol = vols.get_mut(&active).expect("the active volume exists");
    let newest = vol.newest().max(pin);
    (
        vol,
        Volume {
            io,
            newest,
            vol: active,
        },
    )
}

pub struct Fs<D> {
    pub io: Io<D>,
    pub(crate) uuid: [u8; 16],
    block_count: u64,
    label: [u8; 64],
    /// The last commit's generation.
    pub committed: u64,
    /// The writer's clock for the next superblock, nanoseconds since 1970.
    pub clock: u64,
    pub(crate) volumes: Tree,
    pub(crate) vols: BTreeMap<u64, Vol>,
    /// The volume item calls act on.
    pub(crate) active: u64,
    pub(crate) action: Option<Action>,
    writable: bool,
    /// No unknown compat bit is set, so `check --repair` may write.
    repairable: bool,
    pub faults: Faults,
    /// The bits of a name's hash the namespace keeps: all of them on disk; tests
    /// narrow it to make names collide.
    pub hash_mask: u64,
}

impl<D: BlockDev> Fs<D> {
    /// Writes an empty filesystem over all of `dev`, one volume of bare items, as
    /// generation 1.
    pub fn format(dev: D, uuid: [u8; 16], label: &str) -> Result<Self, Error> {
        Self::format_with(dev, uuid, label, Faults::default())
    }

    /// [`Fs::format`] with deliberate faults, for the crash gates.
    pub fn format_with(dev: D, uuid: [u8; 16], label: &str, faults: Faults) -> Result<Self, Error> {
        let mut fs = Self::build(dev, uuid, label, faults, &[(VOLUME, "system")])?;
        fs.commit()?;
        Ok(fs)
    }

    /// An empty filesystem over all of `dev` with `volumes`, not yet committed.
    pub(crate) fn build(
        mut dev: D,
        uuid: [u8; 16],
        label: &str,
        faults: Faults,
        volumes: &[(u64, &str)],
    ) -> Result<Self, Error> {
        let info = dev.info();
        if info.blocks < format::MIN_BLOCKS {
            return Err(Error::NoSpace);
        }
        if !info.durable() {
            return Err(Error::Unsupported("a write-back cache without flush"));
        }
        format::clear_rings(&mut dev, info.blocks)?;
        let mut alloc = Allocator::new(format::data_area(info.blocks));
        alloc.faults.reuse_next = faults.reuse_next;
        let mut name = [0; 64];
        let n = label.len().min(64);
        name[..n].copy_from_slice(&label.as_bytes()[..n]);
        Ok(Fs {
            io: Io {
                dev,
                kind: CsumKind::Seahash64,
                alloc,
                txn: 1,
                node_reads: 0,
                faults,
                deferred: Vec::new(),
            },
            uuid,
            block_count: info.blocks,
            label: name,
            committed: 0,
            clock: 0,
            volumes: Tree::default(),
            vols: volumes
                .iter()
                .map(|(id, name)| (*id, Vol::new(name)))
                .collect(),
            active: volumes[0].0,
            action: None,
            writable: true,
            repairable: true,
            faults,
            hash_mask: u64::MAX,
        })
    }

    /// Mounts the newest commit whose roots verify.
    pub fn mount(dev: D) -> Result<Self, Error> {
        Self::mount_with(dev, Faults::default())
    }

    /// [`Fs::mount`] for a writer with deliberate faults (its reuse wait too).
    pub fn mount_with(mut dev: D, faults: Faults) -> Result<Self, Error> {
        let info = dev.info();
        let found = format::read_rings(&mut dev)?;
        let rules = found[0].rules()?;
        let kind = found[0].csum_kind;
        for sb in found {
            let verifies = |dev: &mut D, ptr: &BlockPtr| {
                ptr.is_null() || format::read_checked(dev, kind, ptr, "root").is_ok()
            };
            if sb.volumes.is_null()
                || !verifies(&mut dev, &sb.volumes)
                || !verifies(&mut dev, &sb.alloc)
            {
                continue;
            }
            let mut fs = Self::open(dev, &sb, faults)?;
            fs.writable = rules.writable && info.durable();
            fs.repairable = rules.repairable;
            return Ok(fs);
        }
        Err(Error::NoValidSuperblock)
    }

    fn open(mut dev: D, sb: &Superblock, faults: Faults) -> Result<Self, Error> {
        let alloc_faults = crate::allocator::Faults {
            reuse_next: faults.reuse_next,
            ..Default::default()
        };
        let alloc = Allocator::load_with(
            &mut dev,
            sb.csum_kind,
            &sb.alloc,
            sb.generation,
            alloc_faults,
        )?;
        let mut io = Io {
            dev,
            kind: sb.csum_kind,
            alloc,
            txn: sb.generation + 1,
            node_reads: 0,
            faults,
            deferred: Vec::new(),
        };
        let volumes = Tree::open(sb.volumes);
        let mut entries: Vec<(Key, Vec<u8>)> = Vec::new();
        volumes.scan(&Key::MIN, &mut Meta(&mut io), &mut |k, v| {
            entries.push((*k, v.to_vec()));
            true
        })?;
        let mut vols: BTreeMap<u64, Vol> = BTreeMap::new();
        let mut snaps: BTreeMap<u64, BTreeMap<u64, Snap>> = BTreeMap::new();
        let mut undone: BTreeSet<u64> = BTreeSet::new();
        for (key, value) in entries {
            match key.kind {
                K_VOLUME if key.off == 0 => {
                    let entry = decode_volume(key.oid, &value)?;
                    let mut vol = Vol::new(&entry.name);
                    vol.live = Tree::open(entry.live.root);
                    vol.live_changed = false;
                    vol.base = (entry.live.root, entry.live.next_oid);
                    vol.next_oid = entry.live.next_oid;
                    vols.insert(key.oid, vol);
                }
                K_SNAPSHOT if key.off > 0 => {
                    snaps
                        .entry(key.oid)
                        .or_default()
                        .insert(key.off, decode_snapshot(&value)?);
                }
                // Read when asked for.
                K_UNDO => {
                    undone.insert(key.oid);
                }
                _ => return Err(Error::Corrupt("an unknown volume table item")),
            }
        }
        let homeless = snaps
            .keys()
            .chain(undone.iter())
            .any(|v| !vols.contains_key(v));
        if vols.is_empty() || homeless {
            return Err(Error::Corrupt("a volume table without its volumes"));
        }
        for (id, list) in snaps {
            let vol = vols
                .get_mut(&id)
                .ok_or(Error::Corrupt("a snapshot of no volume"))?;
            vol.next_snap = list.keys().next_back().map_or(1, |id| id + 1);
            vol.snaps = list;
        }
        let active = match vols.contains_key(&SYSTEM) {
            true => SYSTEM,
            false => *vols.keys().next().ok_or(Error::Corrupt("no volume"))?,
        };
        Ok(Fs {
            io,
            uuid: sb.uuid,
            block_count: sb.block_count,
            label: sb.label,
            committed: sb.generation,
            clock: sb.commit_time,
            volumes,
            vols,
            active,
            action: None,
            writable: true,
            repairable: true,
            faults,
            hash_mask: u64::MAX,
        })
    }

    pub fn into_dev(self) -> D {
        self.io.dev
    }

    pub fn writable(&self) -> bool {
        self.writable
    }

    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    pub fn uuid(&self) -> [u8; 16] {
        self.uuid
    }

    /// Whether `check --repair` may write: no unknown compat bit is set.
    pub fn repairable(&self) -> bool {
        self.repairable
    }

    /// Every volume, by id, with its name.
    pub fn volumes(&self) -> Vec<(u64, String)> {
        self.vols
            .iter()
            .map(|(id, v)| (*id, v.name.clone()))
            .collect()
    }

    /// The volume item calls act on.
    pub fn volume(&self) -> u64 {
        self.active
    }

    /// Makes item calls act on volume `id`.
    pub fn set_volume(&mut self, id: u64) -> Result<(), Error> {
        match self.vols.contains_key(&id) {
            true => {
                self.active = id;
                Ok(())
            }
            false => Err(Error::NotFound),
        }
    }

    pub(crate) fn vol(&self) -> &Vol {
        &self.vols[&self.active]
    }

    pub(crate) fn vol_mut(&mut self) -> &mut Vol {
        self.vols
            .get_mut(&self.active)
            .expect("the active volume exists")
    }

    /// The generation an open action pins, as if it were a snapshot.
    fn pin(&self) -> u64 {
        match (self.action, self.faults.unpinned_action) {
            (Some(action), false) => action.base,
            _ => 0,
        }
    }

    /// The active volume's next object id.
    pub fn next_oid(&self) -> u64 {
        self.vol().next_oid
    }

    /// Hands out a new object id in the active volume; none is ever handed out
    /// twice.
    pub fn alloc_oid(&mut self) -> Result<u64, Error> {
        let vol = self.vol_mut();
        let oid = vol.next_oid;
        vol.next_oid = oid.checked_add(1).ok_or(Error::NoSpace)?;
        vol.live_changed = true;
        Ok(oid)
    }

    /// Raises the counter past `oid`: `check --repair` found an object above it.
    pub(crate) fn bump_oid(&mut self, oid: u64) {
        let vol = self.vol_mut();
        if oid >= vol.next_oid {
            vol.next_oid = oid.saturating_add(1);
            vol.live_changed = true;
        }
    }

    /// Notes that the open action, if any, changed object `oid` of the active
    /// volume: its UNDO record will name it.
    pub(crate) fn touch(&mut self, oid: u64) {
        if self.action.is_some() {
            self.vol_mut().touched.insert(oid);
        }
    }

    /// The raw tree value at `key`: records are named, not read.
    pub fn get_raw(&mut self, key: &Key) -> Result<Option<Vec<u8>>, Error> {
        let pin = self.pin();
        let (vol, mut store) = split(&mut self.vols, &mut self.io, self.active, pin);
        vol.live.get(key, &mut store)
    }

    /// The raw value at `key` in the tree `root` names: a snapshot's.
    pub fn get_raw_in(&mut self, root: &BlockPtr, key: &Key) -> Result<Option<Vec<u8>>, Error> {
        let pin = self.pin();
        let (_, mut store) = split(&mut self.vols, &mut self.io, self.active, pin);
        Tree::open(*root).get(key, &mut store)
    }

    pub fn get(&mut self, key: &Key) -> Result<Option<Value>, Error> {
        match self.get_raw(key)? {
            Some(raw) => decode_value(&raw, &mut self.io).map(Some),
            None => Ok(None),
        }
    }

    /// Sets `key` to inline bytes, at most [`MAX_INLINE`] of them.
    pub fn put(&mut self, key: Key, value: &[u8]) -> Result<(), Error> {
        if value.len() > MAX_INLINE {
            return Err(Error::Invalid("an inline value over the limit"));
        }
        self.set(key, inline_raw(value))
    }

    /// Sets `key` to a record of 1 to [`MAX_RECORD`] bytes, written copy-on-write.
    pub fn put_record(&mut self, key: Key, data: &[u8]) -> Result<(), Error> {
        if data.is_empty() || data.len() > MAX_RECORD {
            return Err(Error::Invalid("a record of no bytes or over 128 KiB"));
        }
        let ptr = self.io.write_record(data)?;
        let mut raw = Vec::with_capacity(RECORD_VALUE);
        raw.push(TAG_RECORD);
        raw.extend_from_slice(&ptr.encode());
        raw.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.set(key, raw)
    }

    /// Sets `key` to a raw value as `get_raw` returned it; `check --repair` and
    /// `restore` move items this way.
    pub(crate) fn set(&mut self, key: Key, raw: Vec<u8>) -> Result<(), Error> {
        let pin = self.pin();
        let (vol, mut store) = split(&mut self.vols, &mut self.io, self.active, pin);
        let old = vol.live.insert(key, raw, &mut store)?;
        vol.live_changed = true;
        release(&mut store, old);
        Ok(())
    }

    /// Removes `key`; returns whether it was there.
    pub fn delete(&mut self, key: &Key) -> Result<bool, Error> {
        let pin = self.pin();
        let (vol, mut store) = split(&mut self.vols, &mut self.io, self.active, pin);
        let old = vol.live.remove(key, &mut store)?;
        let found = old.is_some();
        vol.live_changed |= found;
        release(&mut store, old);
        Ok(found)
    }

    /// Calls `visit` on every raw item from `from` on, in key order, until it
    /// returns false; records are named, not read.
    pub fn scan_raw(
        &mut self,
        from: &Key,
        visit: &mut dyn FnMut(&Key, &[u8]) -> bool,
    ) -> Result<(), Error> {
        let pin = self.pin();
        let (vol, mut store) = split(&mut self.vols, &mut self.io, self.active, pin);
        vol.live.scan(from, &mut store, visit)
    }

    /// Calls `visit` on every raw item of the tree `root` names (a snapshot's, or
    /// the live tree's as last committed) from `from` on, until it returns false.
    pub fn scan_tree(
        &mut self,
        root: &BlockPtr,
        from: &Key,
        visit: &mut dyn FnMut(&Key, &[u8]) -> bool,
    ) -> Result<(), Error> {
        let pin = self.pin();
        let (_, mut store) = split(&mut self.vols, &mut self.io, self.active, pin);
        Tree::open(*root).scan(from, &mut store, visit)
    }

    /// Calls `visit` on every item of the active volume's live tree in a leaf born
    /// after generation `since`, reading only subtrees born after it.
    pub fn scan_born_after(
        &mut self,
        since: u64,
        visit: &mut dyn FnMut(&Key, &[u8]),
    ) -> Result<(), Error> {
        let pin = self.pin();
        let (vol, mut store) = split(&mut self.vols, &mut self.io, self.active, pin);
        vol.live.scan_born_after(since, &mut store, visit)
    }

    /// Makes `items`, raw and in key order, the active volume's whole live tree.
    /// The old tree's blocks are never read again and wait for the GC: `check
    /// --repair` rebuilds a tree it could not wholly read this way.
    pub(crate) fn replace_live(&mut self, items: Vec<(Key, Vec<u8>)>) -> Result<(), Error> {
        let pin = self.pin();
        let (vol, mut store) = split(&mut self.vols, &mut self.io, self.active, pin);
        let mut tree = Tree::default();
        for (key, raw) in items {
            tree.insert(key, raw, &mut store)?;
        }
        vol.live = tree;
        vol.live_changed = true;
        Ok(())
    }

    /// Calls `visit` on every item from `from` on, in key order, until it returns
    /// false; records are read and verified.
    pub fn scan(
        &mut self,
        from: &Key,
        visit: &mut dyn FnMut(&Key, Value) -> bool,
    ) -> Result<(), Error> {
        // In batches, since a record is read through the same `io` the scan uses.
        const BATCH: usize = 256;
        let mut raws: Vec<(Key, Vec<u8>)> = Vec::new();
        let mut cursor = Some(*from);
        while let Some(at) = cursor {
            raws.clear();
            self.scan_raw(&at, &mut |k, v| {
                raws.push((*k, v.to_vec()));
                raws.len() < BATCH
            })?;
            cursor = match raws.len() == BATCH {
                true => raws.last().and_then(|(k, _)| k.next()),
                false => None,
            };
            for (key, raw) in raws.drain(..) {
                if !visit(&key, decode_value(&raw, &mut self.io)?) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Takes a snapshot of the active volume's live tree as it is now: its dirty
    /// nodes are written (no flush), and its root joins the volume table at the
    /// next commit.
    pub fn snapshot(&mut self) -> Result<u64, Error> {
        self.snapshot_as(SnapKind::Named, 0, 0)
    }

    /// [`Fs::snapshot`] of a kind, expiring at `expiry` (0: never), pinned by
    /// action `pin` (0: none).
    pub fn snapshot_as(&mut self, kind: SnapKind, expiry: u64, pin: u64) -> Result<u64, Error> {
        let (txn, active, pinned) = (self.io.txn, self.active, self.pin());
        let (vol, mut store) = split(&mut self.vols, &mut self.io, active, pinned);
        let root = vol.live.write_out(&mut store)?;
        let id = vol.next_snap;
        let snap = Snap {
            root,
            generation: txn,
            next_oid: vol.next_oid,
            kind,
            expiry,
            pin,
        };
        self.volumes.insert(
            snap_key(active, id),
            encode_snapshot(&snap),
            &mut Meta(&mut self.io),
        )?;
        let vol = self.vol_mut();
        vol.next_snap += 1;
        vol.snaps.insert(id, snap);
        Ok(id)
    }

    /// Drops snapshot `id` of the active volume, and the UNDO records an undo
    /// snapshot backs.
    pub fn drop_snapshot(&mut self, id: u64) -> Result<bool, Error> {
        let active = self.active;
        let Some(snap) = self.vol().snaps.get(&id).copied() else {
            return Ok(false);
        };
        if snap.kind == SnapKind::Undo {
            for undo in self
                .undo_records()?
                .into_iter()
                .filter(|u| u.snapshot == id)
            {
                let parts = undo_parts(&undo);
                for part in 0..parts {
                    self.volumes
                        .remove(&undo_key(active, undo.txn, part), &mut Meta(&mut self.io))?;
                }
            }
        }
        self.volumes
            .remove(&snap_key(active, id), &mut Meta(&mut self.io))?;
        self.vol_mut().snaps.remove(&id);
        Ok(true)
    }

    /// Makes snapshot `id`'s tree the active volume's live tree. The live tree's
    /// own blocks wait for the GC; the object id counter never goes back.
    pub fn restore_snapshot(&mut self, id: u64) -> Result<bool, Error> {
        let Some(snap) = self.vol().snaps.get(&id).copied() else {
            return Ok(false);
        };
        let vol = self.vol_mut();
        vol.live = Tree::open(snap.root);
        vol.live_changed = true;
        Ok(true)
    }

    /// The active volume's snapshots.
    pub fn snapshots(&self) -> &BTreeMap<u64, Snap> {
        &self.vol().snaps
    }

    /// Commits the transaction, and the open action with it: two flushes, both
    /// rings. Returns the generation.
    pub fn commit(&mut self) -> Result<u64, Error> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        let generation = self.io.txn;
        if let Some(action) = self.action {
            self.write_undo(action, generation)?;
            self.action = None;
        }
        let mut written = Vec::new();
        for (&id, vol) in self.vols.iter_mut() {
            if !vol.live_changed {
                continue;
            }
            let newest = vol.newest();
            let mut store = Volume {
                io: &mut self.io,
                newest,
                vol: id,
            };
            let root = vol.live.write_out(&mut store)?;
            let entry = VolEntry {
                live: Snap::named(root, generation, vol.next_oid),
                name: vol.name.clone(),
                key_id: 0,
            };
            self.volumes
                .insert(live_key(id), encode_volume(&entry), &mut Meta(&mut self.io))?;
            vol.live_changed = false;
            written.push((id, root));
        }
        let volumes = self.volumes.write_out(&mut Meta(&mut self.io))?;
        for (ptr, vol) in core::mem::take(&mut self.io.deferred) {
            let newest = self.vols.get(&vol).map_or(0, Vol::newest);
            if self.io.reusable(&ptr, newest) {
                self.io.alloc.free_later(generation, run_of(&ptr));
            }
        }
        let head = self
            .io
            .alloc
            .persist(&mut self.io.dev, self.io.kind, generation)?;
        if !self.faults.skip_data_flush {
            self.io.dev.flush()?;
        }
        let mut sb = Superblock::new(self.uuid, self.block_count, generation);
        sb.csum_kind = self.io.kind;
        sb.volumes = volumes;
        sb.alloc = head;
        sb.commit_time = self.clock;
        sb.label = self.label;
        format::write_rings(&mut self.io.dev, &sb)?;
        self.io.dev.flush()?;
        self.committed = generation;
        self.io.txn = generation + 1;
        self.io.alloc.committed(generation);
        for (id, root) in written {
            if let Some(vol) = self.vols.get_mut(&id) {
                vol.base = (root, vol.next_oid);
            }
        }
        Ok(generation)
    }

    /// The active volume's live root as the last commit wrote it, for `check()`:
    /// null when empty, none when it has changed since.
    pub fn live_root(&self) -> Option<BlockPtr> {
        self.live_root_of(self.active)
    }

    /// [`Fs::live_root`] of volume `id`.
    pub fn live_root_of(&self, id: u64) -> Option<BlockPtr> {
        match &self.vols.get(&id)?.live.root {
            None => Some(BlockPtr::NULL),
            Some(crate::tree::Child::Clean(ptr)) => Some(*ptr),
            Some(crate::tree::Child::Dirty(_)) => None,
        }
    }

    /// Volume `id`'s next object id and snapshots.
    pub fn volume_state(&self, id: u64) -> Option<(u64, &BTreeMap<u64, Snap>)> {
        self.vols.get(&id).map(|v| (v.next_oid, &v.snaps))
    }

    pub fn volumes_root(&self) -> Option<BlockPtr> {
        match &self.volumes.root {
            Some(crate::tree::Child::Clean(ptr)) => Some(*ptr),
            _ => None,
        }
    }

    /// Whether nothing has changed since the last commit.
    pub fn is_clean(&self) -> bool {
        self.vols
            .values()
            .all(|v| !v.live_changed && !v.live.is_dirty())
            && !self.volumes.is_dirty()
            && self.io.alloc.is_idle()
    }
}

/// Frees `old`'s record, if it named one, by the reuse rule of `store`'s volume.
fn release<D: BlockDev>(store: &mut Volume<'_, D>, old: Option<Vec<u8>>) {
    if let Some((ptr, _)) = old.as_deref().and_then(record_of) {
        let (newest, vol) = (store.newest, store.vol);
        store.io.replaced_volume_block(ptr, newest, vol);
    }
}
