//! The volume table's entries (ADR 0004 §2, §4-5; P2 task 6): volumes, their
//! snapshots and the UNDO records of the actions that changed them.
//!
//! | Item | Key | Value |
//! |---|---|---|
//! | VOLUME | `(volume, 1, 0)` | the live root, its generation, the next object id, the name and key id |
//! | SNAPSHOT | `(volume, 2, id)` | a root, its generation, the next object id then, its kind, expiry and pin |
//! | UNDO | `(volume, 3, txn << 16 \| part)` | the generation before, the action, the time, the undo snapshot and the objects the action changed |
//!
//! - **Volumes** (§5): `system` (1), `home` (2) and `models` (3); all share one
//!   superblock, so one commit is atomic across them. Task 4's one volume was id
//!   1, which format 1 keeps as `system`.
//! - **Entries grow at the end.** A VOLUME or SNAPSHOT entry written before task
//!   6 stops after the next object id; what follows reads as its default: a
//!   snapshot named by the user, never expiring, pinned by nothing, the volume
//!   named by its id.
//! - **UNDO records** split over parts of up to one item each, so an action may
//!   change any number of objects.

use crate::Error;
use crate::format::{BlockPtr, KIND_NODE, PTR_BYTES};
use crate::tree::{Key, MAX_VALUE};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub const SYSTEM: u64 = 1;
pub const HOME: u64 = 2;
pub const MODELS: u64 = 3;
/// The volumes `mkfs` makes, with their names.
pub const VOLUMES: [(u64, &str); 3] = [(SYSTEM, "system"), (HOME, "home"), (MODELS, "models")];

/// Volume-table item kinds.
pub const K_VOLUME: u8 = 1;
pub const K_SNAPSHOT: u8 = 2;
pub const K_UNDO: u8 = 3;

/// The first object id a volume hands out; 0 is never one.
pub const FIRST_OID: u64 = 1;
/// The longest volume name.
pub const MAX_VOLUME_NAME: usize = 32;
/// How long an undo snapshot lives: 30 days, in nanoseconds (§4).
pub const UNDO_LIFE: u64 = 30 * 24 * 3600 * 1_000_000_000;

const ENTRY_FMT: u8 = 1;
/// A task 4 entry: format byte, root, generation, next object id.
const ENTRY_CORE: usize = 1 + PTR_BYTES + 8 + 8;
/// Task 6's fields after it: kind, expiry, pin, key id, name length.
const ENTRY_TAIL: usize = 1 + 8 + 8 + 8 + 1;
const UNDO_FMT: u8 = 1;
/// An UNDO record's first part: format, before, action, time, snapshot, count.
const UNDO_HEADER: usize = 1 + 8 + 8 + 8 + 8 + 4;
/// Object ids a part holds: the first, and each after it.
const FIRST_PART_OIDS: usize = (MAX_VALUE - UNDO_HEADER) / 8;
const PART_OIDS: usize = (MAX_VALUE - 1) / 8;
/// Parts an UNDO record may have, and generations an UNDO key can name.
const MAX_PARTS: u64 = 1 << 16;
pub const MAX_TXN: u64 = 1 << 48;

/// What a snapshot is for (§4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SnapKind {
    /// The user's, and every snapshot written before task 6.
    Named,
    /// The volume's root before an action (ADR 0002).
    Undo,
    /// Before a new slot's first boot (ADR 0008).
    Update,
    /// Before a package change (ADR 0011).
    Package,
}

impl SnapKind {
    fn code(self) -> u8 {
        match self {
            SnapKind::Named => 1,
            SnapKind::Undo => 2,
            SnapKind::Update => 3,
            SnapKind::Package => 4,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(SnapKind::Named),
            2 => Some(SnapKind::Undo),
            3 => Some(SnapKind::Update),
            4 => Some(SnapKind::Package),
            _ => None,
        }
    }
}

/// A snapshot: its root, null for an empty tree; the generation its tree is
/// from; the volume's next object id then; its kind; when it expires (0: never);
/// and the action whose log entry pins it (0: none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub root: BlockPtr,
    pub generation: u64,
    pub next_oid: u64,
    pub kind: SnapKind,
    pub expiry: u64,
    pub pin: u64,
}

impl Snap {
    pub fn named(root: BlockPtr, generation: u64, next_oid: u64) -> Self {
        Snap {
            root,
            generation,
            next_oid,
            kind: SnapKind::Named,
            expiry: 0,
            pin: 0,
        }
    }
}

/// A volume's own entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolEntry {
    pub live: Snap,
    pub name: String,
    /// The volume's key, 0 in format 1 (P8's encryption, §9).
    pub key_id: u64,
}

/// The name a volume has when its entry does not say.
pub fn default_name(id: u64) -> String {
    VOLUMES
        .iter()
        .find(|(v, _)| *v == id)
        .map_or_else(|| alloc::format!("volume-{id}"), |(_, n)| n.to_string())
}

fn encode(snap: &Snap, key_id: u64, name: &str) -> Vec<u8> {
    let mut value = Vec::with_capacity(ENTRY_CORE + ENTRY_TAIL + name.len());
    value.push(ENTRY_FMT);
    value.extend_from_slice(&snap.root.encode());
    value.extend_from_slice(&snap.generation.to_le_bytes());
    value.extend_from_slice(&snap.next_oid.to_le_bytes());
    value.push(snap.kind.code());
    value.extend_from_slice(&snap.expiry.to_le_bytes());
    value.extend_from_slice(&snap.pin.to_le_bytes());
    value.extend_from_slice(&key_id.to_le_bytes());
    value.push(name.len() as u8);
    value.extend_from_slice(name.as_bytes());
    value
}

pub fn encode_volume(entry: &VolEntry) -> Vec<u8> {
    encode(&entry.live, entry.key_id, &entry.name)
}

pub fn encode_snapshot(snap: &Snap) -> Vec<u8> {
    encode(snap, 0, "")
}

fn word(value: &[u8], at: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&value[at..at + 8]);
    u64::from_le_bytes(word)
}

/// An entry's fields; a task 4 entry reads its tail as defaults.
fn decode(value: &[u8]) -> Result<(Snap, u64, Option<String>), Error> {
    let bad = Error::Corrupt("volume table entry");
    if value.len() < ENTRY_CORE || value[0] != ENTRY_FMT {
        return Err(bad);
    }
    let root = BlockPtr::decode_at(value, 1)?;
    if !root.is_null() && root.kind != KIND_NODE {
        return Err(Error::Corrupt("volume root kind"));
    }
    let next_oid = word(value, 1 + PTR_BYTES + 8);
    if next_oid < FIRST_OID {
        return Err(bad);
    }
    let mut snap = Snap::named(root, word(value, 1 + PTR_BYTES), next_oid);
    let tail = &value[ENTRY_CORE..];
    if tail.is_empty() {
        return Ok((snap, 0, None));
    }
    if tail.len() < ENTRY_TAIL {
        return Err(bad);
    }
    snap.kind = SnapKind::from_code(tail[0]).ok_or(bad)?;
    snap.expiry = word(tail, 1);
    snap.pin = word(tail, 9);
    let key_id = word(tail, 17);
    let len = usize::from(tail[25]);
    let name = tail.get(ENTRY_TAIL..ENTRY_TAIL + len).ok_or(bad)?;
    let name = core::str::from_utf8(name).map_err(|_| bad)?;
    Ok((snap, key_id, Some(name.to_string())))
}

pub fn decode_volume(id: u64, value: &[u8]) -> Result<VolEntry, Error> {
    let (live, key_id, name) = decode(value)?;
    let name = match name {
        Some(name) if !name.is_empty() && name.len() <= MAX_VOLUME_NAME => name,
        Some(_) => return Err(Error::Corrupt("volume name")),
        None => default_name(id),
    };
    if key_id != 0 {
        return Err(Error::Unsupported("an encrypted volume"));
    }
    Ok(VolEntry { live, name, key_id })
}

pub fn decode_snapshot(value: &[u8]) -> Result<Snap, Error> {
    decode(value).map(|(snap, _, _)| snap)
}

pub fn live_key(vol: u64) -> Key {
    Key::new(vol, K_VOLUME, 0)
}

pub fn snap_key(vol: u64, id: u64) -> Key {
    Key::new(vol, K_SNAPSHOT, id)
}

pub fn undo_key(vol: u64, txn: u64, part: u64) -> Key {
    Key::new(vol, K_UNDO, txn << 16 | part)
}

/// What one action did to one volume, as its UNDO record keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undo {
    /// The action's commit.
    pub txn: u64,
    /// The generation before it: the undo snapshot's.
    pub before: u64,
    /// The action, whose log entry pins the record (ADR 0002).
    pub action: u64,
    /// When it committed, by the writer's clock.
    pub time: u64,
    /// The undo snapshot of the volume's root before it.
    pub snapshot: u64,
    /// The objects it changed, in order.
    pub objects: Vec<u64>,
}

/// The record's parts, each a volume-table value of at most one item.
pub fn encode_undo(undo: &Undo) -> Result<Vec<Vec<u8>>, Error> {
    let mut parts = Vec::new();
    let mut first = Vec::with_capacity(MAX_VALUE);
    first.push(UNDO_FMT);
    for field in [undo.before, undo.action, undo.time, undo.snapshot] {
        first.extend_from_slice(&field.to_le_bytes());
    }
    let count = u32::try_from(undo.objects.len())
        .map_err(|_| Error::Invalid("an action that changed too many objects"))?;
    first.extend_from_slice(&count.to_le_bytes());
    let (head, rest) = undo
        .objects
        .split_at(undo.objects.len().min(FIRST_PART_OIDS));
    head.iter()
        .for_each(|o| first.extend_from_slice(&o.to_le_bytes()));
    parts.push(first);
    for chunk in rest.chunks(PART_OIDS) {
        let mut part = Vec::with_capacity(MAX_VALUE);
        part.push(UNDO_FMT);
        chunk
            .iter()
            .for_each(|o| part.extend_from_slice(&o.to_le_bytes()));
        parts.push(part);
    }
    if parts.len() as u64 > MAX_PARTS || undo.txn >= MAX_TXN {
        return Err(Error::Invalid("an action that changed too many objects"));
    }
    Ok(parts)
}

/// An UNDO record from its parts, in order. Each must be whole and the count must
/// match what they hold.
pub fn decode_undo(txn: u64, parts: &[Vec<u8>]) -> Result<Undo, Error> {
    let bad = Error::Corrupt("undo record");
    let first = parts.first().ok_or(bad)?;
    if first.len() < UNDO_HEADER
        || first[0] != UNDO_FMT
        || !(first.len() - UNDO_HEADER).is_multiple_of(8)
    {
        return Err(bad);
    }
    let count = u32::from_le_bytes([first[33], first[34], first[35], first[36]]) as usize;
    let mut objects = Vec::new();
    let mut take = |bytes: &[u8]| {
        let (words, _) = bytes.as_chunks::<8>();
        objects.extend(words.iter().map(|w| u64::from_le_bytes(*w)));
    };
    take(&first[UNDO_HEADER..]);
    for part in &parts[1..] {
        if part.is_empty() || part[0] != UNDO_FMT || !(part.len() - 1).is_multiple_of(8) {
            return Err(bad);
        }
        take(&part[1..]);
    }
    if objects.len() != count {
        return Err(bad);
    }
    Ok(Undo {
        txn,
        before: word(first, 1),
        action: word(first, 9),
        time: word(first, 17),
        snapshot: word(first, 25),
        objects,
    })
}
