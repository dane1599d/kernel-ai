//! `test=blk`: the disk with serial `kai-model` reads back exactly the file xtask
//! attached, through virtio-blk or NVMe, whichever it is on. xtask passes the
//! file's length and FNV-1a hash as `model_len=` and `model_fnv=`; the guest
//! hashes every byte through the driver, including the partial last sector, and
//! the zero padding QEMU adds after it must read as zero. On virtio-blk the disk
//! must be read-only, and every read must reach the staging buffer in place, not
//! through a bounce copy; the NVMe driver has no bounce path at all.

use crate::block::{self, Device};
use crate::virtio::hal;
use crate::{hpet, log};
use kabi::model::{Attached, Fnv1a, SERIAL};

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let attached = Attached::from_cmdline(cmdline);
    let (Some(len), Some(expected)) = (attached.len, attached.fnv) else {
        return Err("blk: the command line has no model_len= and model_fnv=");
    };
    let started = hpet::nanos();
    let shared_before = hal::in_place_shares();
    let mut reads = 0;
    let (kind, sectors, read_only, hash) = block::with_disk(SERIAL, |disk: &mut dyn Device| {
        let mut hash = Fnv1a::new();
        block::stream(disk, len, |_, bytes| {
            reads += 1;
            hash.update(bytes);
            Ok(())
        })
        .map(|()| (disk.kind(), disk.sectors(), disk.read_only(), hash.finish()))
    })
    .ok_or("blk: no disk with serial kai-model")??;
    let in_place = hal::in_place_shares() - shared_before;
    log!(
        "blk: read {len} bytes of {sectors} sectors over {kind} in {} ms, fnv1a64 {hash:016x}",
        (hpet::nanos() - started) / 1_000_000
    );
    // QEMU rounds a raw file up to whole sectors; the test file is not a whole
    // number of them, so this also proves the last sector was partial.
    if sectors != mm::dma::sectors_for(len) || len.is_multiple_of(mm::dma::SECTOR) {
        return Err("blk: the disk is not the file rounded up to a partial last sector");
    }
    // QEMU's NVMe namespaces never report write protection, even on a read-only
    // drive; the NVMe driver has no write command to send.
    if kind == "blk" && !read_only {
        return Err("blk: the kai-model disk is writable");
    }
    log!("blk: {reads} reads, {in_place} buffers shared in place");
    if kind == "blk" && in_place < reads {
        return Err("blk: a read went through a bounce buffer");
    }
    if hash != expected {
        log!("blk: the host's fnv1a64 is {expected:016x}");
        return Err("blk: the guest's hash differs from the host's");
    }
    log!("blk: kai-model reads back byte for byte, partial last sector included");
    Ok(())
}

/// The writable data disk the storage cases attach.
const DATA: &str = "kai-data";

/// A sector's pattern: its number, then bytes that differ from every other's.
fn pattern(sector: u64, out: &mut [u8]) {
    let mut x = sector.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for (index, byte) in out.iter_mut().enumerate() {
        if index % 512 == 0 {
            x ^= sector + (index / 512) as u64;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *byte = x as u8;
    }
}

/// `test=blk-write` (P2 task 1): `kai-data` takes writes at its start, a run of
/// the longest write the driver takes, and its last sector, then a flush; each
/// reads back as written. Writes of part of a sector, past the end or of
/// nothing are refused.
pub fn check_write() -> Result<(), &'static str> {
    block::with_disk(DATA, |disk: &mut dyn Device| {
        if disk.read_only() {
            return Err("blk-write: kai-data is read-only");
        }
        let info = disk.info();
        if !block::durable(info) {
            return Err("blk-write: kai-data cannot make a write durable");
        }
        let (sectors, max) = (disk.sectors(), disk.max_sectors());
        let runs = [(0, 1), (8, max), (sectors - 1, 1)];
        let mut buffer = alloc::vec![0u8; (max * mm::dma::SECTOR) as usize];
        for (sector, count) in runs {
            let bytes = &mut buffer[..(count * mm::dma::SECTOR) as usize];
            pattern(sector, bytes);
            disk.write(sector, bytes)?;
        }
        disk.flush()?;
        for (sector, count) in runs {
            let bytes = &mut buffer[..(count * mm::dma::SECTOR) as usize];
            pattern(sector, bytes);
            if disk.read(sector, count)? != &*bytes {
                return Err("blk-write: a write read back different");
            }
        }
        let refused = [
            disk.write(0, &buffer[..100]).is_err(),
            disk.write(sectors, &buffer[..512]).is_err(),
            disk.write(sectors - 1, &buffer[..1024]).is_err(),
            disk.write(0, &[]).is_err(),
        ];
        if refused.contains(&false) {
            return Err("blk-write: a bad write was not refused");
        }
        log!(
            "blk-write: kai-data over {} takes writes and a flush: {} sectors, {} a write at most, volatile cache {}, atomic {} sectors",
            disk.kind(),
            2 + max,
            max,
            info.volatile_cache,
            info.atomic_sectors
        );
        Ok(())
    })
    .ok_or("blk-write: no disk with serial kai-data")?
}
