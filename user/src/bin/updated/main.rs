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
//! `step=models` exercises the model channel (`models.rs`) for the `models`
//! case: the list, the import of the import disk's fixture, and `model_use`.
//!
//! It prints a line for each step that passed, or `updated: FAIL ...`. With no
//! step, as every boot with a shell plans it (`kabi::plan::with_updated`), it
//! does nothing at boot but settle a model trial left pending. Then, with
//! `serve` in its plan line, it answers the shell's model requests and judges
//! a model on trial; without, it idles: the supervisor would restart an exited
//! service.

#![no_std]
#![no_main]

extern crate alloc;

mod disk;
mod folders;
mod models;
mod restore;
mod window;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use bootslot::{Nv, Record, Slot};
use kabi::Error;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER, RecvInfo};
use kabi::update::{DISK, IMPORT, MAX_IO, NONE, SECTOR, WRITABLE};
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
    // The model channel: a trial the last boot left is settled before anything
    // waits on `llm` (`models.rs`). The plan line's one route is to `llm`.
    let folders = folders::Folders::from_startup().ok();
    let mut channel = folders
        .as_ref()
        .and_then(|f| models::Channel::open(handle, f, user::route(0)));
    if let Some(channel) = &mut channel
        && let Err(why) = channel.at_boot()
    {
        println!("updated: model trial: {why}");
    }
    let result = match (step, reconciled) {
        (_, Err(why)) => Err(why),
        (Some("check"), _) => check(handle),
        (Some("models"), _) => models_check(channel.as_mut()),
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
    serve(channel)
}

/// The `models` case's step: the list, the import disk's model imported, and
/// `model_use` of it for the next restart.
fn models_check(channel: Option<&mut models::Channel>) -> Result<(), String> {
    let channel = channel.ok_or("no model store")?;
    let listed = |channel: &mut models::Channel| {
        let text = channel.answer("models");
        modelchan::listing::parse(&text).map_err(|why| format!("models: {why}: {text}"))
    };
    let before = listed(channel)?;
    println!(
        "updated: models: {} signed, {} stored",
        before.signed.len(),
        before.store.blobs.len()
    );
    let started = channel.answer("models import");
    if !started.starts_with("ok importing ") {
        return Err(format!("models import: {started}"));
    }
    println!("updated: {started}");
    let ended = loop {
        channel.tick();
        let progress = channel.answer("models progress");
        if !progress.starts_with("ok ") || !progress.as_bytes()[3].is_ascii_digit() {
            break progress;
        }
    };
    let Some(id) = ended.strip_prefix("ok imported ") else {
        return Err(format!("models import ended: {ended}"));
    };
    let after = listed(channel)?;
    let sha = after
        .signed
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.sha256)
        .ok_or("the imported model is not in the role")?;
    if !after.store.blobs.iter().any(|b| b.sha256 == sha) {
        return Err(String::from("the imported model is not in the store"));
    }
    let used = channel.answer(&format!("models use {}", comp_models::hex(&sha)));
    if !used.starts_with("ok next ") {
        return Err(format!("models use: {used}"));
    }
    println!("updated: model_use {id}: the next restart uses it, on trial");
    Ok(())
}

/// The process the kernel named `shell` from its plan line, the only one the
/// model requests are answered for (K, 2026-10-30); never pid 0.
fn from_shell(pid: u64) -> bool {
    let Ok(metrics) = sys::metrics() else {
        return false;
    };
    let count = (metrics.process_count as usize).min(metrics.processes.len());
    metrics.processes[..count]
        .iter()
        .any(|p| p.pid == pid && pid != 0 && p.name() == "shell")
}

/// Answers the shell's model requests on `updated`'s endpoint, and between
/// them judges a trial and moves an import on; with no endpoint, only that.
/// Every request is at most [`REQUEST`] bytes and every answer one message.
/// With no model store (no `fs`), each request is answered at once that there
/// is none, so the shell never waits on it.
fn serve(mut channel: Option<models::Channel>) -> ! {
    const REQUEST: usize = 128;
    // One byte more than a request may be: the kernel cuts a longer one to
    // the buffer, so a full buffer means too long.
    let mut request = [0u8; REQUEST + 1];
    loop {
        let busy = channel.as_ref().is_some_and(models::Channel::busy);
        let wait = if busy { 10 } else { 60_000 };
        let mut info = RecvInfo::default();
        match sys::recv_h(&mut request, wait, &mut info) {
            Ok(len) => {
                let count = (info.handle_count as usize).min(MAX_TRANSFER);
                for moved in &info.handles[..count] {
                    let _ = sys::handle_close(Handle(*moved));
                }
                let text = core::str::from_utf8(&request[..len.min(REQUEST)]).unwrap_or("");
                let mut reply = match (from_shell(info.caller_pid), len > REQUEST) {
                    (false, _) => String::from("err only the shell may ask"),
                    (true, true) => String::from("err request too long"),
                    (true, false) => match channel.as_mut() {
                        Some(channel) => channel.answer(text),
                        None => String::from("err there is no model store: no data disk"),
                    },
                };
                let mut end = reply.len().min(kabi::MAX_MESSAGE);
                while !reply.is_char_boundary(end) {
                    end -= 1;
                }
                reply.truncate(end);
                let _ = sys::svc_reply(reply.as_bytes());
            }
            Err(Error::TimedOut) => tick(&mut channel),
            // No `serve` in the plan line (Permission): work without serving.
            Err(Error::NotFound | Error::Permission) => {
                let Some(channel) = channel.as_mut() else {
                    idle()
                };
                loop {
                    channel.tick();
                    sys::sleep_ms(if channel.busy() { 10 } else { 60_000 });
                }
            }
            // Anything else is one receive gone wrong, not the end of serving
            // (K's review of #447).
            Err(why) => {
                println!("updated: a receive failed: {why:?}");
                tick(&mut channel);
                sys::sleep_ms(1_000);
            }
        }
    }
}

fn tick(channel: &mut Option<models::Channel>) {
    if let Some(channel) = channel.as_mut() {
        channel.tick();
    }
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
    import_disk(handle)?;

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

/// The import disk, when one is attached (P4 task 10): SLOT_INFO and SLOT_READ
/// take it as the update disk, and SLOT_WRITE refuses it, as it does the update
/// disk.
fn import_disk(handle: Handle) -> Result<(), String> {
    let Ok(info) = sys::slot_info(handle, IMPORT) else {
        return Ok(());
    };
    let mut first = [0u8; SECTOR as usize];
    sys::slot_read(handle, IMPORT, 0, &mut first).map_err(|e| format!("SLOT_READ: {e:?}"))?;
    if !first.starts_with(&kabi::model::MAGIC) {
        return Err(String::from(
            "the import disk does not start with GGUF's magic",
        ));
    }
    for disk in [IMPORT, DISK] {
        if sys::slot_write(handle, disk, 0, &first) != Err(Error::Permission) {
            return Err(format!("SLOT_WRITE did not refuse disk {disk}"));
        }
    }
    println!(
        "updated: the import disk reads, {} sectors of a GGUF file, and SLOT_WRITE refuses it",
        info.sectors
    );
    Ok(())
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
