//! Partitions and files (lane S-io, ADR 0004 §6-7).
//! - PART_INFO, PART_READ, PART_WRITE and PART_FLUSH each need a `Partition`
//!   handle, which only the `fs` service holds, and reach only the partition it
//!   names: the kernel checks every range against that partition, so a block
//!   outside it, the ESP and the slots included, can never be named.
//! - ENDPOINT_MINT lets a server make a `Dir` or `File` handle on its own
//!   endpoint, with a badge it chooses, to hand to a client; a call on it reaches
//!   that server like a route's.

use crate::block::{self, Device};
use crate::handle::{self, grant::Kernel};
use crate::process::uaccess::{copy_from_user, copy_to_user};
use alloc::vec::Vec;
use captable::Entry;
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
    call::PART_PEER_ALIVE,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::PART_INFO => info(slot, a[0], a[1]),
        call::PART_READ => read(slot, a[0], a[1], a[2], a[3]),
        call::PART_WRITE => write(slot, a[0], a[1], a[2], a[3]),
        call::PART_FLUSH => flush(slot, a[0]),
        call::PART_PEER_ALIVE => peer_alive(slot, a[0], a[1]),
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
    read_in(slot, kind, sector, buf, len)
}

/// Reads `len` bytes from `sector` of the partition of type `kind` into the
/// caller's `buf`: PART_READ's work, and SLOT_READ's (`update.rs`).
pub(super) fn read_in(
    slot: usize,
    kind: diskmap::Guid,
    sector: u64,
    buf: u64,
    len: u64,
) -> Result<u64, Error> {
    let mut data = Vec::new();
    data.try_reserve_exact(len.min(MAX_IO) as usize)
        .map_err(|_| Error::NoMemory)?;
    on(kind, |disk| {
        let count = range(sector, len, disk.sectors())?;
        let step = disk.max_sectors().max(1);
        let mut at = sector;
        while at < sector + count {
            let n = step.min(sector + count - at);
            let bytes = disk.read(at, n).map_err(|_| Error::Unavailable)?;
            data.extend_from_slice(&bytes[..(n * SECTOR) as usize]);
            at += n;
        }
        Ok::<(), Error>(())
    })??;
    // Copied out once the disk is unlocked: a fault on `buf` never holds it.
    copy_to_user(slot, buf, &data)?;
    Ok(len)
}

fn write(slot: usize, raw: u64, sector: u64, buf: u64, len: u64) -> Result<u64, Error> {
    let kind = partition(slot, raw, Rights::WRITE)?;
    write_in(slot, kind, sector, buf, len)
}

/// Writes `len` bytes of the caller's `buf` from `sector` of the partition of
/// type `kind`: PART_WRITE's work, and SLOT_WRITE's (`update.rs`).
pub(super) fn write_in(
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
        .map_err(|_| Error::NoMemory)?;
    data.resize(len as usize, 0);
    copy_from_user(slot, buf, &mut data)?;
    on(kind, |disk| {
        range(sector, len, disk.sectors())?;
        if disk.read_only() {
            return Err(Error::Permission);
        }
        let step = disk.max_sectors().max(1);
        for (index, chunk) in data.chunks((step * SECTOR) as usize).enumerate() {
            let at = sector + index as u64 * step;
            disk.write(at, chunk).map_err(|_| Error::Unavailable)?;
        }
        Ok(len)
    })?
}

fn flush(slot: usize, raw: u64) -> Result<u64, Error> {
    let kind = partition(slot, raw, Rights::WRITE)?;
    flush_in(kind)
}

/// Makes every write to the disk of the partition of type `kind` durable.
pub(super) fn flush_in(kind: diskmap::Guid) -> Result<u64, Error> {
    on(kind, |disk| {
        if disk.read_only() {
            return Err(Error::Permission);
        }
        disk.flush().map_err(|_| Error::Unavailable)?;
        Ok(0)
    })?
}

/// Whether `pid` still runs, for the holder of a `Partition` handle (`fs`).
fn peer_alive(slot: usize, raw: u64, pid: u64) -> Result<u64, Error> {
    partition(slot, raw, Rights::empty())?;
    match pid == 0 || crate::process::name_of(pid).is_some() {
        true => Ok(0),
        false => Err(Error::NotFound),
    }
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
