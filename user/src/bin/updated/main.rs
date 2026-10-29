//! The update service (ADR 0008 §10), the only holder of an `Update` handle:
//! the slots R, A and B, the boot record and the update disk.
//!
//! `step=update` (`disk.rs`) takes the update from the update disk: a TUF
//! repository, checked by `kaituf` against the root this slot was built with,
//! whose target `slot.img` it streams into the other slot, hashing as it
//! writes; a target that does not match is refused and its slot never boots.
//!
//! `step=check` exercises the calls for the `update-stage` case:
//!
//! 1. SLOT_INFO for each slot, and the slot this boot runs in;
//! 2. SLOT_WRITE refused for the active slot, and for R while it is a trial;
//! 3. BOOTREC_SET refused for a record whose CRC fails, and for a one-shot that
//!    names a slot being written;
//! 4. *stage*: the other slot marked Updating, the active slot copied into it
//!    in 128 KiB writes, flushed, read back and compared, then a trial of
//!    release 2;
//! 5. *restart into* it: BOOTREC_SET with the restart flag.
//!
//! It prints a line for each step that passed, or `updated: FAIL ...`, then
//! idles: the supervisor would restart an exited service.

#![no_std]
#![no_main]

extern crate alloc;

mod disk;
mod folders;
mod restore;
mod window;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use bootslot::{Nv, Record, Slot};
use kabi::Error;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP};
use kabi::update::{MAX_IO, NONE, SECTOR, WRITABLE};
use user::{println, sys};

user::entry!(main);

/// How long to wait for the boot lever's record.
const LEVER_WAIT_MS: u64 = 120_000;
/// The release the check stages.
const STAGED_RELEASE: u32 = 2;
/// Sectors between short sleeps while copying: the disk calls do not block, and
/// a process that runs 5 s without blocking is killed as a runaway.
const YIELD_EVERY: u64 = 2 * 1024 * 1024 / SECTOR;

fn main() -> i32 {
    let mut args = [0u8; 128];
    let len = sys::args(&mut args).unwrap_or(0);
    let args = core::str::from_utf8(&args[..len]).unwrap_or("");
    let step = args
        .split_whitespace()
        .find_map(|w| w.strip_prefix("step="));
    let Some(handle) = startup_update() else {
        println!("updated: FAIL no Update handle at startup");
        idle();
    };
    // The kernel's boot lever writes the record first; until it has, the boot
    // record answers Busy.
    for _ in 0..LEVER_WAIT_MS / 100 {
        if sys::bootrec_get(handle) != Err(Error::Busy) {
            break;
        }
        sys::sleep_ms(100);
    }
    // Every start first settles an update left in flight (`restore.rs`).
    let reconciled = restore::reconcile(handle);
    if let Ok(outcome) = reconciled
        && outcome != restore::Outcome::Nothing
    {
        println!("updated: reconciled: {outcome:?}");
    }
    let result = match (step, reconciled) {
        (_, Err(why)) => Err(why),
        (Some("check"), _) => check(handle),
        (Some("update"), _) => {
            let now = !args.split_whitespace().any(|w| w == "restart=window");
            disk::update(handle, now).map(|staged| {
                if !now {
                    window::wait(handle, staged, args);
                }
            })
        }
        // The rollback case: the reconcile is the step, then the boot ends.
        (Some("reconcile"), Ok(_)) => {
            if args.split_whitespace().any(|w| w == "after=restart") {
                restart_here(handle);
            }
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(why) = result {
        println!("updated: FAIL {why}");
        // In the update cases, a refused update ends the boot too, back into
        // the slot that is running, so the next boot shows what it left.
        if args.split_whitespace().any(|w| w == "after=restart") {
            restart_here(handle);
        }
    }
    idle()
}

/// Restarts into the slot this boot runs in, with the record as it stands.
fn restart_here(handle: Handle) {
    let (Ok(record), Ok(info)) = (record(handle), sys::slot_info(handle, 1)) else {
        return;
    };
    println!("updated: restarting into the running slot");
    let _ = sys::bootrec_set(handle, &record.encode(), info.active, true);
}

pub(crate) fn idle() -> ! {
    loop {
        sys::sleep_ms(60_000);
    }
}

fn startup_update() -> Option<Handle> {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut handles).ok()?;
    handles[..count.min(MAX_STARTUP)]
        .iter()
        .find(|h| h.kind() == Some(Kind::Update))
        .map(|h| Handle(h.handle))
}

pub(crate) fn slot_of(raw: u64) -> Result<Slot, String> {
    Slot::ALL
        .into_iter()
        .find(|s| *s as u64 == raw)
        .ok_or_else(|| format!("no slot {raw}"))
}

pub(crate) fn record(handle: Handle) -> Result<Record, String> {
    let bytes = sys::bootrec_get(handle).map_err(|e| format!("BOOTREC_GET: {e:?}"))?;
    Record::decode(&bytes)
        .ok_or_else(|| String::from("BOOTREC_GET gave a record that does not decode"))
}

fn check(handle: Handle) -> Result<(), String> {
    let info = |slot| sys::slot_info(handle, slot).map_err(|e| format!("SLOT_INFO {slot}: {e:?}"));
    let (r, a, b) = (info(0)?, info(1)?, info(2)?);
    let active = slot_of(a.active)?;
    let other = active.other().ok_or("this boot runs in R")?;
    println!(
        "updated: slots R {} A {} B {} sectors; this boot runs in {active:?}",
        r.sectors, a.sectors, b.sectors
    );
    let sector = [0u8; SECTOR as usize];
    if sys::slot_write(handle, active as u64, 0, &sector) != Err(Error::Permission) {
        return Err(String::from("SLOT_WRITE took the active slot"));
    }
    let committed = record(handle)?.slot(active).successful;
    let r_write = sys::slot_write(handle, 0, 0, &sector);
    if !committed && (r_write != Err(Error::Permission) || r.flags & WRITABLE != 0) {
        return Err(String::from("SLOT_WRITE took R from a trial slot"));
    }
    println!("updated: SLOT_WRITE refuses the active slot, and R from a trial slot");

    let mut nv = Nv {
        record: record(handle)?,
        one_shot: Some(active),
    };
    let mut bad = nv.record.encode();
    bad[10] ^= 0xFF;
    if sys::bootrec_set(handle, &bad, NONE, false) != Err(Error::Invalid) {
        return Err(String::from("BOOTREC_SET took a record whose CRC fails"));
    }
    bootslot::stage_begin(&mut nv, other);
    if sys::bootrec_set(handle, &nv.record.encode(), other as u64, false) != Err(Error::Invalid) {
        return Err(String::from("BOOTREC_SET armed a slot being written"));
    }
    set(handle, &nv, false)?;
    println!("updated: BOOTREC_SET refuses a bad record and arming a slot being written");

    let sectors = info(active as u64)?
        .sectors
        .min(info(other as u64)?.sectors);
    copy(handle, active as u64, other as u64, sectors)?;
    bootslot::stage_end(&mut nv, other, STAGED_RELEASE);
    bootslot::restart_into(&mut nv, other);
    println!(
        "updated: staged {other:?} as release {STAGED_RELEASE}: {} MiB written, flushed and read back",
        (sectors * SECTOR) >> 20
    );
    println!("updated: restarting into {other:?}");
    set(handle, &nv, true)
}

pub(crate) fn set(handle: Handle, nv: &Nv, restart: bool) -> Result<(), String> {
    let one_shot = nv.one_shot.map_or(NONE, |s| s as u64);
    sys::bootrec_set(handle, &nv.record.encode(), one_shot, restart)
        .map(|_| ())
        .map_err(|e| format!("BOOTREC_SET: {e:?}"))
}

/// Copies `sectors` sectors of slot `from` into slot `to`, flushes, and reads
/// them back.
fn copy(handle: Handle, from: u64, to: u64, sectors: u64) -> Result<(), String> {
    let step = MAX_IO / SECTOR;
    let mut buf = vec![0u8; MAX_IO as usize];
    let mut back = vec![0u8; MAX_IO as usize];
    let mut at = 0;
    while at < sectors {
        let n = step.min(sectors - at);
        let bytes = (n * SECTOR) as usize;
        sys::slot_read(handle, from, at, &mut buf[..bytes])
            .map_err(|e| format!("SLOT_READ: {e:?}"))?;
        sys::slot_write(handle, to, at, &buf[..bytes]).map_err(|e| format!("SLOT_WRITE: {e:?}"))?;
        at += n;
        pause(at);
    }
    sys::slot_flush(handle, to).map_err(|e| format!("SLOT_FLUSH: {e:?}"))?;
    let mut at = 0;
    while at < sectors {
        let n = step.min(sectors - at);
        let bytes = (n * SECTOR) as usize;
        sys::slot_read(handle, from, at, &mut buf[..bytes])
            .map_err(|e| format!("SLOT_READ: {e:?}"))?;
        sys::slot_read(handle, to, at, &mut back[..bytes])
            .map_err(|e| format!("SLOT_READ: {e:?}"))?;
        if buf[..bytes] != back[..bytes] {
            return Err(format!("slot {to} reads back differently at sector {at}"));
        }
        at += n;
        pause(at);
    }
    Ok(())
}

/// Blocks for a moment every [`YIELD_EVERY`] sectors.
pub(crate) fn pause(at: u64) {
    if at.is_multiple_of(YIELD_EVERY) {
        sys::sleep_ms(1);
    }
}
