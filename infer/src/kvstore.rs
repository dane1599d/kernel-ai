//! Where a preempted request's KV state is kept (ADR 0013 section 2, the agents
//! OS plan's F4): the mechanism under `agentsched`'s placement. `agentsched`
//! decides each move (a [`Place`] to another); [`KvStore`] makes it on the
//! engine:
//!
//! - **RAM**: a [`Snapshot`] in the pool, within its byte budget ([`pool_budget`]:
//!   what is free once the model is loaded, up to [`MAX_POOL_BYTES`]);
//! - **spill**: the `kai-kv 1` file ([`crate::segments`]) in a raw [`Region`],
//!   which the OS backs with the `kai-cache` region (the founder, 2026-11-06). A
//!   RAM snapshot the pool demotes is written there as the same file;
//! - **cold**: nothing kept; the next run prefills again.
//!
//! A restore is checked as a file's or a snapshot's always is: anything that does
//! not hold leaves the slot empty, and the run prefills. A state restored or
//! dropped gives its pool bytes or its region room back.

use crate::chat::{Engine, Turn};
use crate::segments::{self, Key, Refused, Snapshot};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// The pool's ceiling (ADR 0013 question 4): 512 MiB.
pub const MAX_POOL_BYTES: u64 = 512 << 20;
/// Bytes a spill reads or writes at a time.
const STEP: usize = 64 << 10;

/// The pool for `free_after_load` bytes of memory free once the model is in:
/// all of it up to [`MAX_POOL_BYTES`] (the plan's default for small machines).
pub fn pool_budget(free_after_load: u64) -> u64 {
    free_after_load.min(MAX_POOL_BYTES)
}

/// Where a state is (`agentsched::Place`, as the engine sees it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// In a slot of the engine: nothing to keep.
    Slot,
    Ram,
    Spill,
    Cold,
}

/// Raw bytes a spill file goes to: the `kai-cache` region in the OS, a buffer
/// in the tests. Each call is whole or fails, and moves whole blocks at a
/// block's offset: the store aligns every file to [`Region::block`] and pads
/// its last block.
pub trait Region {
    /// The bytes every offset and length is a multiple of: a sector for a
    /// disk. It divides 64 KiB, the most the store moves at once.
    fn block(&self) -> u64 {
        1
    }
    /// Its bytes.
    fn size(&self) -> u64;
    fn read_at(&mut self, at: u64, buf: &mut [u8]) -> bool;
    fn write_at(&mut self, at: u64, bytes: &[u8]) -> bool;
    /// Makes what was written durable.
    fn flush(&mut self) -> bool;
}

/// Why a state was not kept where it was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unkept {
    /// The pool has no room for it: the policy should have demoted first.
    PoolFull,
    /// No region, or no room in it.
    NoRoom,
    /// The engine or the region failed while copying it.
    Failed,
    /// It is not where the move says it is, or the move goes nowhere.
    Elsewhere,
    /// Coming back, it did not hold for the prompt: the slot is left empty, and
    /// the run prefills.
    Refused(Refused),
}

/// One move of `agentsched`'s (`Decision::Move { request, from, to }`), in the
/// engine's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub request: u64,
    pub from: Place,
    pub to: Place,
}

/// What a move came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The state left the slot, or the pool, for where the move sent it.
    Kept,
    /// The state is back in the slot in use: the positions the prompt reuses,
    /// 0 for one kept nowhere.
    Restored(usize),
}

/// The key a request's kept state is checked by, as `agentsched`'s engine rig
/// keys it: `model`'s file and tier, the agent and the request as its pack id,
/// and slot 0, since the state is bound to its request and to no slot.
pub fn request_key(model: &Key, agent: u32, request: u64) -> Key {
    Key {
        pack_id: segments::spill_pack_id(agent, request),
        slot: 0,
        ..*model
    }
}

/// A spill file's place in the region: `room` bytes from `at`, whole blocks,
/// the file first and zeros after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Extent {
    at: u64,
    room: u64,
}

pub struct KvStore<R: Region> {
    budget: u64,
    ram: BTreeMap<u64, Snapshot>,
    spills: BTreeMap<u64, Extent>,
    region: Option<R>,
}

impl<R: Region> KvStore<R> {
    /// A store with a pool of `budget` bytes, spilling to `region` if any.
    pub fn new(budget: u64, region: Option<R>) -> Self {
        Self {
            budget,
            ram: BTreeMap::new(),
            spills: BTreeMap::new(),
            region,
        }
    }

    /// Bytes the pool's snapshots hold.
    pub fn ram_bytes(&self) -> u64 {
        self.ram.values().map(|s| s.bytes() as u64).sum()
    }

    /// Where `request`'s state is: in the pool, spilled, or (as far as the store
    /// knows) in a slot or nowhere.
    pub fn place(&self, request: u64) -> Option<Place> {
        match (
            self.ram.contains_key(&request),
            self.spills.contains_key(&request),
        ) {
            (true, _) => Some(Place::Ram),
            (_, true) => Some(Place::Spill),
            _ => None,
        }
    }

    /// Makes `mv` for agent `agent`'s request, whose prompt is `turns`, on the
    /// engine's slot in use, which the caller has switched to the request's
    /// (out) or to a free one (in). States are keyed by [`request_key`].
    pub fn apply(
        &mut self,
        engine: &mut Engine<'_>,
        model: &Key,
        mv: Move,
        agent: u32,
        turns: &[Turn<'_>],
    ) -> Result<Applied, Unkept> {
        let key = request_key(model, agent, mv.request);
        match (mv.from, mv.to) {
            (Place::Slot, Place::Slot) => Err(Unkept::Elsewhere),
            (Place::Slot, to) => self
                .park(engine, &key, mv.request, to)
                .map(|()| Applied::Kept),
            (Place::Ram, to @ (Place::Spill | Place::Cold)) => {
                self.demote(mv.request, to).map(|()| Applied::Kept)
            }
            (Place::Spill, Place::Cold) => {
                self.forget(mv.request);
                Ok(Applied::Kept)
            }
            (_, Place::Slot) => self
                .restore(engine, &key, mv.request, turns)
                .map(Applied::Restored)
                .map_err(Unkept::Refused),
            _ => Err(Unkept::Elsewhere),
        }
    }

    /// Moves `request`'s state out of the engine's slot in use, which holds it
    /// under `key`, to `to`. To [`Place::Cold`] keeps nothing. On an error
    /// nothing is kept, and the slot still holds the state.
    pub fn park(
        &mut self,
        engine: &Engine<'_>,
        key: &Key,
        request: u64,
        to: Place,
    ) -> Result<(), Unkept> {
        self.forget(request);
        match to {
            Place::Slot | Place::Cold => Ok(()),
            Place::Ram => {
                let mut snapshot = Snapshot::new();
                engine
                    .snapshot_slot(key, &mut snapshot)
                    .map_err(|_| Unkept::Failed)?;
                if self.ram_bytes() + snapshot.bytes() as u64 > self.budget {
                    return Err(Unkept::PoolFull);
                }
                self.ram.insert(request, snapshot);
                Ok(())
            }
            Place::Spill => {
                let len = engine.slot_file_bytes() as u64;
                self.spill(request, len, |out| engine.save_slot(key, out).is_ok())
            }
        }
    }

    /// Moves `request`'s RAM snapshot to `to`: [`Place::Spill`], the same file
    /// written from the snapshot, or [`Place::Cold`]. Its pool bytes come back
    /// either way; a spill that fails leaves it cold.
    pub fn demote(&mut self, request: u64, to: Place) -> Result<(), Unkept> {
        let snapshot = self.ram.remove(&request).ok_or(Unkept::Elsewhere)?;
        match to {
            Place::Spill => {
                let len = segments::snapshot_file_bytes(&snapshot) as u64;
                self.spill(request, len, |out| {
                    segments::save_snapshot(&snapshot, out).is_ok()
                })
            }
            _ => Ok(()),
        }
    }

    /// Brings `request`'s state back into the engine's slot in use for a prompt
    /// of `turns`: the positions that prompt reuses, 0 for a state kept nowhere
    /// (the run prefills). Kept state is let go either way; a refused one leaves
    /// the slot empty, as a cold start.
    pub fn restore(
        &mut self,
        engine: &mut Engine<'_>,
        key: &Key,
        request: u64,
        turns: &[Turn<'_>],
    ) -> Result<usize, Refused> {
        if let Some(snapshot) = self.ram.remove(&request) {
            return engine.restore_snapshot(key, turns, &snapshot);
        }
        let Some(extent) = self.spills.remove(&request) else {
            return Ok(0);
        };
        let Some(region) = self.region.as_mut() else {
            return Err(Refused::Short);
        };
        // Whole steps of blocks are read, and handed out as the engine asks.
        let end = extent.at + extent.room;
        let mut at = extent.at;
        let mut chunk: Vec<u8> = Vec::new();
        let mut used = 0;
        engine.restore_slot(key, turns, &mut |out| {
            let mut done = 0;
            while done < out.len() {
                if used == chunk.len() {
                    let step = (end - at).min(STEP as u64) as usize;
                    chunk.resize(step, 0);
                    if step == 0 || !region.read_at(at, &mut chunk) {
                        return false;
                    }
                    (at, used) = (at + step as u64, 0);
                }
                let take = (out.len() - done).min(chunk.len() - used);
                out[done..done + take].copy_from_slice(&chunk[used..used + take]);
                (done, used) = (done + take, used + take);
            }
            true
        })
    }

    /// Forgets whatever is kept for `request`.
    pub fn forget(&mut self, request: u64) {
        self.ram.remove(&request);
        self.spills.remove(&request);
    }

    /// Writes a file of `len` bytes for `request`, from `write`, into the first
    /// room in the region that fits it, and makes it durable.
    fn spill(
        &mut self,
        request: u64,
        len: u64,
        write: impl FnOnce(&mut dyn FnMut(&[u8]) -> bool) -> bool,
    ) -> Result<(), Unkept> {
        let block = self.region.as_ref().map_or(1, Region::block);
        if block == 0 || !(STEP as u64).is_multiple_of(block) {
            return Err(Unkept::NoRoom);
        }
        let room = len.div_ceil(block) * block;
        let at = self.room(room).ok_or(Unkept::NoRoom)?;
        let region = self.region.as_mut().ok_or(Unkept::NoRoom)?;
        let end = at + room;
        let mut next = at;
        let mut pending: Vec<u8> = Vec::with_capacity(STEP);
        let put = |bytes: &[u8], region: &mut R, next: &mut u64| {
            let fits = *next + bytes.len() as u64 <= end;
            let written = fits && region.write_at(*next, bytes);
            *next += bytes.len() as u64;
            written
        };
        // Whole steps go out as they fill, so every write is whole blocks.
        let wrote = write(&mut |bytes| {
            pending.extend_from_slice(bytes);
            let mut ok = true;
            while ok && pending.len() >= STEP {
                ok = put(&pending[..STEP], region, &mut next);
                pending.drain(..STEP);
            }
            ok
        });
        // The last block is padded with zeros, which a restore never reads as file.
        pending.resize(pending.len().div_ceil(block as usize) * block as usize, 0);
        let ok = wrote
            && (pending.is_empty() || put(&pending, region, &mut next))
            && next == end
            && region.flush();
        if !ok {
            return Err(Unkept::Failed);
        }
        self.spills.insert(request, Extent { at, room });
        Ok(())
    }

    /// The lowest offset with `len` free bytes in the region: extents start at
    /// whole blocks, since each takes whole blocks from one.
    fn room(&self, len: u64) -> Option<u64> {
        let size = self.region.as_ref()?.size();
        let mut taken: Vec<Extent> = self.spills.values().copied().collect();
        taken.sort_by_key(|e| e.at);
        let mut at = 0;
        for e in taken {
            if e.at.saturating_sub(at) >= len {
                return Some(at);
            }
            at = at.max(e.at + e.room);
        }
        (size.saturating_sub(at) >= len).then_some(at)
    }
}
