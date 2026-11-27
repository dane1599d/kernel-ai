//! Partitions and files (lane S-io, ADR 0004 §6-7).
//! - PART_INFO, PART_READ, PART_WRITE and PART_FLUSH each need a `Partition`
//!   handle, which only the `fs` service holds, and reach only the partition it
//!   names: the kernel checks every range against that partition, so a block
//!   outside it, the ESP and the slots included, can never be named.
//! - ENDPOINT_MINT lets a server make a `Dir` or `File` handle on its own
//!   endpoint, with a badge it chooses, to hand to a client; a call on it reaches
//!   that server like a route's. ENDPOINT_BADGE reads such a handle's badge and
//!   rights back, for its own endpoint only: one a client moved to it.

use crate::block::{self, Device};
use crate::handle::{self, grant::Kernel};
use crate::process::uaccess::{copy_from_user, copy_to_user};
use alloc::vec::Vec;
use captable::Entry;
use core::sync::atomic::{AtomicU32, Ordering};
use kabi::handle::{Handle, Kind, Rights};
use kabi::part::{self, MAX_IO, SECTOR};
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::PART_INFO,
    call::PART_READ,
    call::PART_WRITE,
    call::PART_FLUSH,
    call::ENDPOINT_MINT,
    call::ENDPOINT_BADGE,
    call::PART_PEER_ALIVE,
    call::PART_PEER_NAME,
    call::PART_PEER_CAPS,
    call::PART_SELF,
    call::MODEL_FILL,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::PART_INFO => info(slot, a[0], a[1]),
        call::PART_READ => read(slot, a[0], a[1], a[2], a[3]),
        call::PART_WRITE => write(slot, a[0], a[1], a[2], a[3]),
        call::PART_FLUSH => flush(slot, a[0]),
        call::PART_PEER_ALIVE => peer_alive(slot, a[0], a[1]),
        call::PART_PEER_NAME => peer_name(slot, a[0], a[1], a[2], a[3]),
        call::PART_PEER_CAPS => peer_caps(slot, a[0], a[1]),
        call::PART_SELF => part_self(slot, a[0]),
        call::MODEL_FILL => model_fill(slot, a[0], a[1], a[2], a[3]),
        call::ENDPOINT_BADGE => endpoint_badge(slot, a[0], a[1]),
        _ => mint(slot, a[0], a[1], a[2], a[3]),
    })
}

/// The partition type the object of a checked `Partition` handle stands for.
fn partition(slot: usize, raw: u64, rights: Rights) -> Result<diskmap::Guid, Error> {
    let handle = Handle(u32::try_from(raw).map_err(|_| Error::BadHandle)?);
    let entry = handle::with(slot, |h| {
        h.table.check(handle, Kind::Partition, rights, &Kernel)
    })?
    .map_err(Error::from)?;
    match entry.object {
        part::DATA => Ok(diskmap::KAI_DATA_TYPE),
        _ => Err(Error::NotFound),
    }
}

/// Failed PART_* calls so far: [`failed`] logs the first few, then each power of
/// two, so a dead disk that fails every call does not fill the log.
static FAILED: AtomicU32 = AtomicU32::new(0);

/// Logs a failed disk call (`call`: "part read", "slot write", ...) with its
/// cause, which the caller sees only as an error number: `why` is the disk's
/// own message when it gave one. Every kind is logged but Invalid and
/// Permission, which are `fs`'s own range and rights checks, answered at once
/// to a caller that sees them (logging them used up the unthrottled lines
/// before the first bench); a kind added to these paths later shows. Called outside the disk's lock, from the syscall (never
/// from an interrupt), and only on failure.
fn failed(call: &str, sector: u64, len: u64, error: Error, why: &str) -> Error {
    if matches!(error, Error::Invalid | Error::Permission) {
        return error;
    }
    let count = FAILED.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    if count <= 8 || count.is_power_of_two() {
        crate::log!(
            "{call} failed: {error:?}{}{why} (sector {sector}, {len} bytes; failure {count})",
            if why.is_empty() { "" } else { ", " }
        );
    }
    error
}

/// Runs `f` on the partition, NotFound when the disk has none of that type.
pub(super) fn on<R>(kind: diskmap::Guid, f: impl FnOnce(&mut dyn Device) -> R) -> Result<R, Error> {
    block::with_partition(kind, f).ok_or(Error::NotFound)
}

/// The sectors `len` bytes from `sector` cover, when they are whole sectors, 1 to
/// [`MAX_IO`] bytes, and wholly inside a partition of `sectors`.
fn range(sector: u64, len: u64, sectors: u64) -> Result<u64, Error> {
    if len == 0 || len > MAX_IO || !len.is_multiple_of(SECTOR) {
        return Err(Error::Invalid);
    }
    let count = len / SECTOR;
    match sector.checked_add(count) {
        Some(end) if end <= sectors => Ok(count),
        _ => Err(Error::Invalid),
    }
}

fn info(slot: usize, raw: u64, out: u64) -> Result<u64, Error> {
    let kind = partition(slot, raw, Rights::INSPECT)?;
    let (sectors, writable, volatile) = on(kind, |disk| {
        (
            disk.sectors(),
            !disk.read_only(),
            disk.info().volatile_cache,
        )
    })?;
    let flags =
        if writable { part::WRITABLE } else { 0 } | if volatile { part::VOLATILE_CACHE } else { 0 };
    let mut bytes = [0u8; core::mem::size_of::<part::PartInfo>()];
    for (field, value) in
        bytes
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .zip([sectors, MAX_IO / SECTOR, flags])
    {
        *field = value.to_le_bytes();
    }
    copy_to_user(slot, out, &bytes)?;
    Ok(0)
}

fn read(slot: usize, raw: u64, sector: u64, buf: u64, len: u64) -> Result<u64, Error> {
    let kind = partition(slot, raw, Rights::READ)?;
    read_in("part read", slot, kind, sector, buf, len)
}

/// Reads `len` bytes from `sector` of the partition of type `kind` into the
/// caller's `buf`: PART_READ's work, and SLOT_READ's (`update.rs`).
pub(super) fn read_in(
    call: &str,
    slot: usize,
    kind: diskmap::Guid,
    sector: u64,
    buf: u64,
    len: u64,
) -> Result<u64, Error> {
    let mut data = Vec::new();
    data.try_reserve_exact(len.min(MAX_IO) as usize)
        .map_err(|_| failed(call, sector, len, Error::NoMemory, "the heap has no room"))?;
    let mut why = "";
    let read = on(kind, |disk| {
        let count = range(sector, len, disk.sectors())?;
        let step = disk.max_sectors().max(1);
        let mut at = sector;
        while at < sector + count {
            let n = step.min(sector + count - at);
            let bytes = disk.read(at, n).map_err(|message| {
                why = message;
                Error::Unavailable
            })?;
            data.extend_from_slice(&bytes[..(n * SECTOR) as usize]);
            at += n;
        }
        Ok::<(), Error>(())
    })
    .and_then(|done| done);
    read.map_err(|error| failed(call, sector, len, error, why))?;
    // Copied out once the disk is unlocked: a fault on `buf` never holds it.
    copy_to_user(slot, buf, &data)
        .map_err(|error| failed(call, sector, len, error, "copying out to the caller"))?;
    Ok(len)
}

fn write(slot: usize, raw: u64, sector: u64, buf: u64, len: u64) -> Result<u64, Error> {
    let kind = partition(slot, raw, Rights::WRITE)?;
    write_in("part write", slot, kind, sector, buf, len)
}

/// Writes `len` bytes of the caller's `buf` from `sector` of the partition of
/// type `kind`: PART_WRITE's work, and SLOT_WRITE's (`update.rs`).
pub(super) fn write_in(
    call: &str,
    slot: usize,
    kind: diskmap::Guid,
    sector: u64,
    buf: u64,
    len: u64,
) -> Result<u64, Error> {
    if len == 0 || len > MAX_IO {
        return Err(Error::Invalid);
    }
    let mut data = Vec::new();
    data.try_reserve_exact(len as usize)
        .map_err(|_| failed(call, sector, len, Error::NoMemory, "the heap has no room"))?;
    data.resize(len as usize, 0);
    copy_from_user(slot, buf, &mut data)
        .map_err(|error| failed(call, sector, len, error, "copying in from the caller"))?;
    let mut why = "";
    let wrote = on(kind, |disk| {
        range(sector, len, disk.sectors())?;
        if disk.read_only() {
            return Err(Error::Permission);
        }
        let step = disk.max_sectors().max(1);
        for (index, chunk) in data.chunks((step * SECTOR) as usize).enumerate() {
            let at = sector + index as u64 * step;
            disk.write(at, chunk).map_err(|message| {
                why = message;
                Error::Unavailable
            })?;
        }
        Ok(len)
    })
    .and_then(|done| done);
    wrote.map_err(|error| failed(call, sector, len, error, why))
}

fn flush(slot: usize, raw: u64) -> Result<u64, Error> {
    let kind = partition(slot, raw, Rights::WRITE)?;
    flush_in("part flush", kind)
}

/// Makes every write to the disk of the partition of type `kind` durable.
pub(super) fn flush_in(call: &str, kind: diskmap::Guid) -> Result<u64, Error> {
    let mut why = "";
    let flushed = on(kind, |disk| {
        if disk.read_only() {
            return Err(Error::Permission);
        }
        disk.flush().map_err(|message| {
            why = message;
            Error::Unavailable
        })?;
        Ok(0)
    })
    .and_then(|done| done);
    flushed.map_err(|error| failed(call, 0, 0, error, why))
}

/// Whether `pid` still runs, for the holder of a `Partition` handle (`fs`).
fn peer_alive(slot: usize, raw: u64, pid: u64) -> Result<u64, Error> {
    partition(slot, raw, Rights::empty())?;
    match pid == 0 || crate::process::name_of(pid).is_some() {
        true => Ok(0),
        false => Err(Error::NotFound),
    }
}

/// PART_PEER_NAME: live process `pid`'s name from the table, under its lock,
/// so `fs` matches a `/system/apps` open to the caller exactly however many
/// processes run (`kabi::Metrics` lists only the first few).
fn peer_name(slot: usize, raw: u64, pid: u64, ptr: u64, cap: u64) -> Result<u64, Error> {
    partition(slot, raw, Rights::empty())?;
    let (name, _) = crate::process::name_of(pid).ok_or(Error::NotFound)?;
    let (bytes, len) = name.bytes();
    if cap < len as u64 {
        return Err(Error::TooBig);
    }
    copy_to_user(slot, ptr, &bytes[..len])?;
    Ok(len as u64)
}

/// PART_PEER_CAPS: live process `pid`'s capability bits from the table, under
/// its lock, for `fs` alone (a `Partition` handle). Pid 0, the kernel, has
/// none: NotFound, as for a process that is not running.
pub(crate) fn peer_caps(slot: usize, raw: u64, pid: u64) -> Result<u64, Error> {
    partition(slot, raw, Rights::empty())?;
    crate::process::caps_of(pid).ok_or(Error::NotFound)
}

/// PART_SELF: the caller's own pid, for `fs` alone (a `Partition` handle),
/// which tells a caller older than itself that a restart dropped its action.
fn part_self(slot: usize, raw: u64) -> Result<u64, Error> {
    partition(slot, raw, Rights::empty())?;
    Ok(crate::process::identity(slot).pid)
}

/// MODEL_FILL: the model store's model, from `kai-data` block runs the caller
/// names, into the runs reserved at boot ([`crate::model::fill`]).
fn model_fill(slot: usize, raw: u64, ptr: u64, count: u64, offset: u64) -> Result<u64, Error> {
    let kind = partition(slot, raw, Rights::READ)?;
    if kind != diskmap::KAI_DATA_TYPE {
        return Err(Error::Invalid);
    }
    let count = usize::try_from(count)
        .ok()
        .filter(|&n| n <= crate::model::fill::MAX_EXTENTS)
        .ok_or(Error::Invalid)?;
    let mut raw = [0u8; 16 * crate::model::fill::MAX_EXTENTS];
    copy_from_user(slot, ptr, &mut raw[..16 * count])?;
    let extents: Vec<(u64, u64)> = raw[..16 * count]
        .as_chunks::<16>()
        .0
        .iter()
        .map(|pair| {
            let (first, blocks) = pair.split_at(8);
            (
                u64::from_le_bytes(first.try_into().unwrap()),
                u64::from_le_bytes(blocks.try_into().unwrap()),
            )
        })
        .collect();
    // `test=model-store fill=...`: the restart and give-up cases run first.
    crate::model_fill_tests::before_fill(&extents, offset);
    crate::model::fill::fill(&extents, offset)
}

/// Rights a minted handle of each server-backed kind can carry.
const DIR_RIGHTS: Rights = Rights::DUPLICATE
    .union(Rights::TRANSFER)
    .union(Rights::INSPECT)
    .union(Rights::READ)
    .union(Rights::WRITE)
    .union(Rights::ENUMERATE)
    .union(Rights::CREATE)
    .union(Rights::DELETE);
const FILE_RIGHTS: Rights = Rights::DUPLICATE
    .union(Rights::TRANSFER)
    .union(Rights::INSPECT)
    .union(Rights::READ)
    .union(Rights::WRITE);

/// ENDPOINT_MINT: a `Dir` or `File` handle on the caller's own endpoint (which
/// must carry MINT), routed to the caller's service name, with `badge`.
pub(crate) fn mint(
    slot: usize,
    endpoint: u64,
    kind: u64,
    rights: u64,
    badge: u64,
) -> Result<u64, Error> {
    let endpoint = Handle(u32::try_from(endpoint).map_err(|_| Error::BadHandle)?);
    let kind = u32::try_from(kind).ok().and_then(Kind::from_raw);
    let allowed = match kind {
        Some(Kind::Dir) => DIR_RIGHTS,
        Some(Kind::File) => FILE_RIGHTS,
        _ => return Err(Error::Invalid),
    };
    // Masked to the kind's rights, so MINT, REVOKE or CALL are never minted;
    // nothing left is refused.
    let rights = Rights::from_bits_truncate(rights as u32) & allowed;
    if rights.is_empty() {
        return Err(Error::Invalid);
    }
    let me = crate::process::identity(slot);
    // Its own endpoint only: one handed to it names another process. Checked
    // before the route name is taken, so a refused call takes no name slot.
    handle::with(slot, |h| {
        let own = h
            .table
            .check(endpoint, Kind::Endpoint, Rights::MINT, &Kernel)?;
        match own.object == me.pid {
            true => Ok(()),
            false => Err(captable::Error::Permission),
        }
    })?
    .map_err(Error::from)?;
    let route = handle::route_id(me.name.as_str())?;
    // Tied to no grant node (`grant: None`, as `Entry::new` makes it): it lives
    // until closed. Revoking a server's handles is later work (ADR 0001's
    // "badges revoked" notice).
    let mut entry = Entry::new(kind.ok_or(Error::Invalid)?, rights, route);
    entry.badge = badge;
    let handle = handle::with(slot, |h| h.table.insert(entry))?.map_err(Error::from)?;
    Ok(u64::from(handle.0))
}

/// ENDPOINT_BADGE: [`badge_of`] written to the caller's `out`.
fn endpoint_badge(slot: usize, raw: u64, out: u64) -> Result<u64, Error> {
    let (badge, rights) = badge_of(slot, raw)?;
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&badge.to_le_bytes());
    bytes[8..].copy_from_slice(&u64::from(rights.bits()).to_le_bytes());
    copy_to_user(slot, out, &bytes)?;
    Ok(0)
}

/// The badge and rights of `raw`, a `Dir` or `File` handle in the caller's
/// table that names the caller's own endpoint (a route by its own name, as
/// [`mint`] makes them); Permission for any other; BadHandle for none, or a
/// raw value past u32; Revoked for one whose grant was revoked. It takes no
/// route name: a caller without one is simply refused.
pub(crate) fn badge_of(slot: usize, raw: u64) -> Result<(u64, Rights), Error> {
    let handle = Handle(u32::try_from(raw).map_err(|_| Error::BadHandle)?);
    // `live`, as CALL_H takes a Dir or File: a handle whose grant was revoked
    // answers Revoked, though it is not closed yet.
    let entry = handle::with(slot, |h| h.table.live(handle, &Kernel))?.map_err(Error::from)?;
    if !matches!(entry.kind, Kind::Dir | Kind::File) {
        return Err(Error::Permission);
    }
    let me = crate::process::identity(slot);
    let own =
        handle::route_name(entry.object).is_some_and(|name| name.as_str() == me.name.as_str());
    match own {
        true => Ok((entry.badge, entry.rights)),
        false => Err(Error::Permission),
    }
}
