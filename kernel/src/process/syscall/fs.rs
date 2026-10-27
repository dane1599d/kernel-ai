//! Partitions (lane S-io, ADR 0004 §6-7): PART_INFO, PART_READ, PART_WRITE and
//! PART_FLUSH. Each needs a `Partition` handle, which only the `fs` service holds,
//! and reaches only the partition it names: the kernel checks every range against
//! that partition, so a block outside it, the ESP and the slots included, can never
//! be named.

use crate::block::{self, Device};
use crate::handle::{self, grant::Kernel};
use crate::process::uaccess::{copy_from_user, copy_to_user};
use alloc::vec::Vec;
use kabi::handle::{Handle, Kind, Rights};
use kabi::part::{self, MAX_IO, SECTOR};
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::PART_INFO,
    call::PART_READ,
    call::PART_WRITE,
    call::PART_FLUSH,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::PART_INFO => info(slot, a[0], a[1]),
        call::PART_READ => read(slot, a[0], a[1], a[2], a[3]),
        call::PART_WRITE => write(slot, a[0], a[1], a[2], a[3]),
        _ => flush(slot, a[0]),
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
fn on<R>(kind: diskmap::Guid, f: impl FnOnce(&mut dyn Device) -> R) -> Result<R, Error> {
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
    on(kind, |disk| {
        if disk.read_only() {
            return Err(Error::Permission);
        }
        disk.flush().map_err(|_| Error::Unavailable)?;
        Ok(0)
    })?
}
