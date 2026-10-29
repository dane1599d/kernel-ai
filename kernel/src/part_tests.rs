//! `test=part` (P2 task 3, ADR 0004 §6-7): the `fs` service and its `Partition`.
//! - `fs` is the only process holding a `Partition` handle, and every one it
//!   holds names `kai-data`: no handle names the ESP or the system slots.
//! - Inside `fs` (`check`): sector 0 of its partition is `kai-data`'s first, not
//!   the disk's, a range past the end is refused, and a handle narrowed to READ
//!   can neither write nor flush.
//! - S04-2's figures re-run on the real calls: a 32 MiB file goes to `fs` in
//!   7 KiB messages and comes back, through kaifs (`/system/s04-2`, one action
//!   committed at the end) and the PART_* calls.
//!
//! The kernel is `fs`'s client here, so the figures include no ring-3 client's
//! own cost; S04-2 measured that, with the file in `fs`'s memory.

use crate::block::{self, Device};
use crate::{handle, hpet, ipc, log, process};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::handle::Kind;
use kabi::part::{SECTOR, stub};

const SERVICE: &str = "fs";
const CALL_MS: u64 = 30_000;
/// What the kernel puts in `kai-data`'s first sector for `fs` to find there.
const MARKER: &[u8; 16] = b"kai-data sector0";
/// The file S04-2's figures move: 32 MiB.
const FILE_BYTES: u64 = 32 << 20;

pub fn check() -> Result<(), &'static str> {
    // The marker is in place before `fs` can look.
    block::with_partition(diskmap::KAI_DATA_TYPE, |disk: &mut dyn Device| {
        let mut sector = [0u8; SECTOR as usize];
        sector[..MARKER.len()].copy_from_slice(MARKER);
        disk.write(0, &sector)
    })
    .ok_or("part: no kai-data partition")?
    .map_err(|_| "part: the marker could not be written")?;
    // `check` writes the partition's last sector, which is in kaifs's second
    // superblock ring: what was there goes back once the write is seen.
    let sectors = block::with_partition(diskmap::KAI_DATA_TYPE, |disk: &mut dyn Device| {
        disk.sectors()
    })
    .unwrap_or(0);
    let saved = block::with_partition(diskmap::KAI_DATA_TYPE, |disk: &mut dyn Device| {
        disk.read(sectors.saturating_sub(1), 1).map(<[u8]>::to_vec)
    })
    .and_then(Result::ok)
    .ok_or("part: kai-data's last sector cannot be read")?;
    let reply = ask(stub::CHECK)?;
    let first: String = MARKER.iter().map(|b| format!("{b:02x}")).collect();
    let want = format!("ok sectors={sectors} first={first}");
    if reply != want.as_bytes() {
        log!("part: fs answered {:?}", String::from_utf8_lossy(&reply));
        return Err("part: fs's check failed, or it reached another partition");
    }
    last_sector(sectors)?;
    block::with_partition(diskmap::KAI_DATA_TYPE, |disk: &mut dyn Device| {
        disk.write(sectors - 1, &saved[..SECTOR as usize])
            .and_then(|()| disk.flush())
    })
    .and_then(Result::ok)
    .ok_or("part: kai-data's last sector could not be put back")?;
    only_fs()?;
    log!("part: fs alone holds a Partition, for kai-data only, and its checks pass");
    bench()
}

fn ask(request: &[u8]) -> Result<Vec<u8>, &'static str> {
    ipc::call(SERVICE, request, CALL_MS).map_err(|_| "part: the call to fs failed")
}

/// What `fs` wrote and flushed to its last sector is `kai-data`'s last sector.
fn last_sector(sectors: u64) -> Result<(), &'static str> {
    let ok = block::with_partition(diskmap::KAI_DATA_TYPE, |disk: &mut dyn Device| {
        disk.read(sectors - 1, 1)
            .is_ok_and(|bytes| bytes.starts_with(b"fs: last sector!"))
    });
    if ok != Some(true) {
        return Err("part: fs's write to its last sector is not kai-data's last sector");
    }
    Ok(())
}

/// No process but `fs` holds a `Partition` handle.
fn only_fs() -> Result<(), &'static str> {
    let holders: Vec<String> = handle::holders(Kind::Partition)
        .into_iter()
        .map(|slot| String::from(process::identity(slot).name.as_str()))
        .collect();
    if holders != [SERVICE] {
        log!("part: Partition handles are held by {holders:?}");
        return Err("part: a process other than fs holds a Partition");
    }
    Ok(())
}

/// The bytes of the file at `offset`, a pattern `fs` cannot guess.
fn pattern(offset: u64, out: &mut [u8]) {
    for (i, byte) in out.iter_mut().enumerate() {
        let at = offset + i as u64;
        *byte = (at ^ (at >> 11) ^ (at >> 19)).wrapping_mul(0x9E) as u8;
    }
}

/// S04-2's 64 MiB transfer, on 32 MiB of the disk.
fn bench() -> Result<(), &'static str> {
    let step = stub::MAX_DATA as u64;
    let mut request = Vec::with_capacity(9 + stub::MAX_DATA);
    let started = hpet::nanos();
    let mut at = 0;
    while at < FILE_BYTES {
        let n = step.min(FILE_BYTES - at);
        request.clear();
        request.push(stub::WRITE);
        request.extend_from_slice(&at.to_le_bytes());
        let start = request.len();
        request.resize(start + n as usize, 0);
        pattern(at, &mut request[start..]);
        if ask(&request)? != b"ok" {
            return Err("part: fs refused a write");
        }
        at += n;
    }
    if ask(&[stub::FLUSH])? != b"ok" {
        return Err("part: fs refused the flush");
    }
    let wrote = hpet::nanos() - started;
    let started = hpet::nanos();
    let mut want = alloc::vec![0u8; stub::MAX_DATA];
    let mut at = 0;
    while at < FILE_BYTES {
        let n = step.min(FILE_BYTES - at);
        request.clear();
        request.push(stub::READ);
        request.extend_from_slice(&at.to_le_bytes());
        request.extend_from_slice(&(n as u32).to_le_bytes());
        let got = ask(&request)?;
        pattern(at, &mut want[..n as usize]);
        if got != want[..n as usize] {
            return Err("part: the file read back different");
        }
        at += n;
    }
    let read = hpet::nanos() - started;
    let rate = |ns: u64| FILE_BYTES * 1_000 / ns.max(1);
    log!(
        "part: S04-2 on the real calls: {} MiB in {} B messages, write {} MB/s (to the flush), read {} MB/s, {} cpus",
        FILE_BYTES >> 20,
        stub::MAX_DATA,
        rate(wrote),
        rate(read),
        crate::smp::online()
    );
    Ok(())
}
