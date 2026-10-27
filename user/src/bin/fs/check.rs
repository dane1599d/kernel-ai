//! `check`, the `part` case's half inside `fs`: what the `Partition` handle
//! reaches and what it is refused.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use kabi::Error;
use kabi::handle::{Handle, Rights};
use kabi::part::{MAX_IO, SECTOR, WRITABLE};
use user::sys;

/// What the last sector holds after `check`, which the kernel reads back.
pub const LAST: &[u8; 16] = b"fs: last sector!";

/// Runs every check; `ok sectors=N first=HEX` or `fail <why>`.
pub fn run(part: Handle) -> String {
    match checks(part) {
        Ok(line) => line,
        Err(why) => format!("fail {why}"),
    }
}

fn checks(part: Handle) -> Result<String, String> {
    let info = sys::part_info(part).map_err(|e| format!("PART_INFO: {e:?}"))?;
    let n = info.sectors;
    if n == 0 || info.flags & WRITABLE == 0 || info.max_sectors != MAX_IO / SECTOR {
        return Err(format!("PART_INFO reports {info:?}"));
    }
    let mut sector = vec![0u8; SECTOR as usize];
    sys::part_read(part, 0, &mut sector).map_err(|e| format!("reading sector 0: {e:?}"))?;
    let first: String = sector[..16].iter().map(|b| format!("{b:02x}")).collect();
    // The last sector takes a write, a flush, and reads back.
    sector.fill(0);
    sector[..LAST.len()].copy_from_slice(LAST);
    sys::part_write(part, n - 1, &sector).map_err(|e| format!("writing sector N-1: {e:?}"))?;
    sys::part_flush(part).map_err(|e| format!("PART_FLUSH: {e:?}"))?;
    let mut back = vec![0u8; SECTOR as usize];
    sys::part_read(part, n - 1, &mut back).map_err(|e| format!("reading sector N-1: {e:?}"))?;
    if back != sector {
        return Err(String::from("the last sector read back different"));
    }
    refusals(part, n)?;
    narrowed(part)?;
    Ok(format!("ok sectors={n} first={first}"))
}

/// Ranges outside the partition, and lengths that are not whole sectors or are
/// too long, are refused with Invalid; nothing past its end can be named.
fn refusals(part: Handle, n: u64) -> Result<(), String> {
    let mut big = vec![0u8; (MAX_IO + SECTOR) as usize];
    let s = SECTOR as usize;
    let reads: [(&str, u64, usize); 6] = [
        ("one sector past the end", n, s),
        ("two sectors across the end", n - 1, 2 * s),
        ("sector u64::MAX", u64::MAX, s),
        ("no bytes", 0, 0),
        ("part of a sector", 0, s - 1),
        ("more than MAX_IO", 0, big.len()),
    ];
    for (what, at, len) in reads {
        let got = sys::part_read(part, at, &mut big[..len]);
        if got != Err(Error::Invalid) {
            return Err(format!("a read of {what} gave {got:?}"));
        }
    }
    let got = sys::part_write(part, n, &big[..s]);
    if got != Err(Error::Invalid) {
        return Err(format!("a write past the end gave {got:?}"));
    }
    Ok(())
}

/// A handle narrowed to READ and INSPECT reads but cannot write or flush, and a
/// closed one reaches nothing.
fn narrowed(part: Handle) -> Result<(), String> {
    let reader = sys::handle_dup(part, Rights::READ | Rights::INSPECT)
        .map_err(|e| format!("HANDLE_DUP: {e:?}"))?;
    let mut sector = vec![0u8; SECTOR as usize];
    sys::part_read(reader, 0, &mut sector).map_err(|e| format!("a READ handle read: {e:?}"))?;
    for (what, got) in [
        ("write", sys::part_write(reader, 0, &sector)),
        ("flush", sys::part_flush(reader)),
    ] {
        if got != Err(Error::Permission) {
            return Err(format!("a {what} through a READ handle gave {got:?}"));
        }
    }
    sys::handle_close(reader).map_err(|e| format!("HANDLE_CLOSE: {e:?}"))?;
    let got = sys::part_read(reader, 0, &mut sector);
    if got != Err(Error::BadHandle) {
        return Err(format!("a read through a closed handle gave {got:?}"));
    }
    Ok(())
}
