//! An update from the update disk (ADR 0008 §12): the disk holds a copy of the
//! TUF repository as a `kai-bundle 1` (`metadata/<role>.json` and
//! `targets/slot.img`, under its own path since a bundle path is at most 64
//! bytes), and `updated` checks it exactly as it will a
//! download from P5 on: `kaituf` refreshes from the root this slot was built
//! with, finds `slot.img`, and the release it names must be newer than every
//! release this machine has committed. The image then streams into the other
//! slot, marked Updating first so a half-written slot never boots, its SHA-256
//! computed on the way; it becomes a trial only if length and hash match.

use crate::folders::{self, Folders};
use crate::{pause, record, restore, set, slot_of};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bootslot::Nv;
use kabi::handle::Handle;
use kabi::update::{DISK, MAX_IO, SECTOR};
use kaituf::Error as TufError;
use kaituf::updater::{Cache, Remote, Updater};
use sha2::{Digest, Sha256};
use user::{println, sys};

/// The target an update disk carries.
pub const TARGET: &str = "slot.img";
/// The most a root may be, as `kaituf`'s limit.
const ROOT_MAX: usize = kaituf::updater::ROOT_MAX as usize;

/// The update disk's bundle table, read once.
struct Repo {
    handle: Handle,
    table: Vec<u8>,
    total: u64,
}

impl Repo {
    fn open(handle: Handle) -> Result<Repo, String> {
        let info = sys::slot_info(handle, DISK).map_err(|e| format!("no update disk: {e:?}"))?;
        let total = info.sectors * SECTOR;
        let head = read(handle, 0, kaibundle::HEADER_BYTES as u64)?;
        let size = kaibundle::table_bytes(&head).map_err(|e| format!("update disk: {e:?}"))?;
        let table = read(handle, 0, size as u64)?;
        Ok(Repo {
            handle,
            table,
            total,
        })
    }

    fn entry(&self, path: &str) -> Option<kaibundle::TableEntry<'_>> {
        let count = (self.table.len() - kaibundle::HEADER_BYTES) / kaibundle::ENTRY_BYTES;
        (0..count)
            .filter_map(|i| kaibundle::table_entry(&self.table, i, self.total).ok())
            .find(|e| e.path == path)
    }
}

/// `len` bytes from byte `offset` (a sector boundary) of the update disk.
fn read(handle: Handle, offset: u64, len: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; MAX_IO as usize];
    let (mut at, end) = (offset, offset + len);
    while at < end {
        let want = (end - at).min(MAX_IO).next_multiple_of(SECTOR);
        let chunk = &mut buf[..want as usize];
        sys::slot_read(handle, DISK, at / SECTOR, chunk)
            .map_err(|e| format!("reading the update disk: {e:?}"))?;
        let take = (end - at).min(want) as usize;
        out.extend_from_slice(&chunk[..take]);
        at += take as u64;
    }
    Ok(out)
}

impl Remote for Repo {
    fn fetch(&mut self, name: &str, max: u64) -> Result<Option<Vec<u8>>, TufError> {
        let Some(entry) = self.entry(&format!("metadata/{name}")) else {
            return Ok(None);
        };
        if entry.length > max {
            return Err(TufError::TooLarge(name.into()));
        }
        let (offset, length) = (entry.offset, entry.length);
        read(self.handle, offset, length)
            .map(Some)
            .map_err(TufError::Io)
    }
}

/// The verified metadata: `/system/trust/<role>.json` when `kai-data` is there
/// (ADR 0008 §7's cache, so the next refresh starts from the newest root and
/// refuses replayed metadata), and this refresh's copies in memory. The root
/// starts as the one in `/system/trust`, or else the slot's own.
struct Trust {
    memory: BTreeMap<String, Vec<u8>>,
    dir: Option<Handle>,
}

impl Cache for Trust {
    fn load(&mut self, role: &str) -> Option<Vec<u8>> {
        if let Some(bytes) = self.memory.get(role) {
            return Some(bytes.clone());
        }
        let bytes = folders::read(self.dir?, &format!("{role}.json")).ok()??;
        self.memory.insert(role.into(), bytes.clone());
        Some(bytes)
    }

    fn save(&mut self, role: &str, bytes: &[u8]) -> Result<(), TufError> {
        self.memory.insert(role.into(), bytes.into());
        match self.dir {
            Some(dir) => folders::write(dir, &format!("{role}.json"), bytes).map_err(TufError::Io),
            None => Ok(()),
        }
    }
}

/// `step=update`.
pub fn update(handle: Handle) -> Result<(), String> {
    let mut root = vec![0u8; ROOT_MAX];
    let len = sys::trust_root(handle, &mut root).map_err(|e| format!("TRUST_ROOT: {e:?}"))?;
    root.truncate(len);
    let now = sys::trust_time(handle).map_err(|e| format!("TRUST_TIME: {e:?}"))?;
    let mut repo = Repo::open(handle)?;
    let dirs = Folders::from_startup();
    let mut cache = Trust {
        memory: BTreeMap::new(),
        dir: dirs.trust,
    };
    let cached = dirs
        .trust
        .and_then(|dir| folders::read(dir, "root.json").ok().flatten());
    match cached {
        Some(newest) => {
            println!("updated: the trusted root is /system/trust's");
            cache.memory.insert("root".into(), newest);
        }
        None => {
            println!("updated: the trusted root is the slot's own");
            cache.save("root", &root).map_err(|e| format!("{e}"))?;
        }
    }
    let (target, file) = {
        let mut updater = Updater::new(&mut repo, &mut cache, now).map_err(|e| format!("{e}"))?;
        updater.refresh().map_err(|e| format!("refused: {e}"))?;
        let target = updater
            .target(TARGET)
            .map_err(|e| format!("refused: {e}"))?
            .ok_or("refused: the repository has no slot.img")?;
        let file = format!("targets/{}", target.path);
        (target, file)
    };
    let release_seq = target.custom["kai"]["release_seq"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("refused: slot.img names no release_seq")?;
    println!(
        "updated: the update disk's repository verifies: {TARGET} is release {release_seq}, {} bytes",
        target.length
    );
    let want = target
        .hashes
        .iter()
        .find(|(alg, _)| alg == "sha256")
        .map(|(_, digest)| digest.clone())
        .ok_or("refused: slot.img has no sha256")?;
    let info = sys::slot_info(handle, 1).map_err(|e| format!("SLOT_INFO: {e:?}"))?;
    let active = slot_of(info.active)?;
    let other = active.other().ok_or("this boot runs in R")?;
    let mut nv = Nv {
        record: record(handle)?,
        one_shot: Some(active),
    };
    let newest = nv
        .record
        .slot(active)
        .release_seq
        .max(nv.record.min_release_seq);
    if release_seq <= newest {
        return Err(format!(
            "refused: release {release_seq} is not newer than {newest} (anti-rollback)"
        ));
    }
    let entry = repo
        .entry(&file)
        .ok_or_else(|| format!("refused: the update disk has no {file}"))?;
    let (offset, length) = (entry.offset, entry.length);
    let room = sys::slot_info(handle, other as u64)
        .map_err(|e| format!("SLOT_INFO: {e:?}"))?
        .sectors
        * SECTOR;
    if length != target.length || length > room {
        return Err(format!(
            "refused: {file} is {length} bytes, the target {} and the slot {room}",
            target.length
        ));
    }
    bootslot::stage_begin(&mut nv, other);
    set(handle, &nv, false)?;
    let got = stream(handle, offset, length, other as u64)?;
    if got[..] != want[..] {
        // The slot stays Updating: it never boots, and the next update rewrites it.
        return Err(format!(
            "refused: {TARGET} does not match its signed sha256; {other:?} stays unbootable"
        ));
    }
    println!("updated: wrote release {release_seq} into {other:?} and checked its sha256");
    if dirs.trust.is_some() {
        println!("updated: the verified metadata is in /system/trust");
    }
    // Before the new slot's first boot: if it fails before it commits, the old
    // slot boots on this snapshot of `system` (ADR 0008 §10). `home` is never in it.
    if let Some(updates) = dirs.updates {
        let id = folders::update_snapshot(updates)?;
        println!("updated: took update snapshot {id} of the system volume");
        let pending = restore::Pending {
            snapshot: id,
            slot: other,
            release_seq,
        };
        restore::record_pending(updates, pending)?;
    }
    bootslot::stage_end(&mut nv, other, release_seq);
    bootslot::restart_into(&mut nv, other);
    println!("updated: restarting into {other:?}");
    set(handle, &nv, true)
}

/// Copies `length` bytes from `offset` of the update disk into slot `to` and
/// flushes it, then reads the slot back and returns the SHA-256 of what it
/// holds: the read-back hash ADR 0008's *stage* asks for.
fn stream(handle: Handle, offset: u64, length: u64, to: u64) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; MAX_IO as usize];
    let mut done = 0;
    while done < length {
        let want = (length - done).min(MAX_IO);
        let chunk = &mut buf[..want.next_multiple_of(SECTOR) as usize];
        sys::slot_read(handle, DISK, (offset + done) / SECTOR, chunk)
            .map_err(|e| format!("reading the update disk: {e:?}"))?;
        // A last partial sector is written whole, zero past the image's end.
        chunk[want as usize..].fill(0);
        sys::slot_write(handle, to, done / SECTOR, chunk)
            .map_err(|e| format!("SLOT_WRITE: {e:?}"))?;
        done += want;
        pause(done / SECTOR);
    }
    sys::slot_flush(handle, to).map_err(|e| format!("SLOT_FLUSH: {e:?}"))?;
    let mut hash = Sha256::new();
    let mut done = 0;
    while done < length {
        let want = (length - done).min(MAX_IO);
        let chunk = &mut buf[..want.next_multiple_of(SECTOR) as usize];
        sys::slot_read(handle, to, done / SECTOR, chunk)
            .map_err(|e| format!("SLOT_READ: {e:?}"))?;
        hash.update(&chunk[..want as usize]);
        done += want;
        pause(done / SECTOR);
    }
    Ok(hash.finalize().to_vec())
}
