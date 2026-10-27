//! The boot lever (ADR 0008 section 4): on a slot image, each boot reads
//! `KaiBootState`, decides with `bootslot`, and writes the record and Limine's
//! `LoaderEntryOneShot` through `efi.rs`.
//!
//! - In R: what `bootslot::r_boots` arms, after a fault or a lost boot; with no
//!   record, a fresh install (`bootslot::install`) from R's own `kai-slot 1`.
//! - In A or B: once the required checks pass (the supervisor running and the
//!   shell serving), *up*. A fault from then on goes through [`on_fault`], which
//!   records it and deletes the one-shot, so R boots next.
//!
//! A boot is on a slot image when its entry's command line names `slot=`
//! (`r`, `a` or `b`): Limine 12 reports partition 0 for a file loaded through
//! `boot(N):`, so the file's own partition cannot tell (decisions.md, S08-1).
//! The dev loop's directory ESP names no slot, and nothing here runs.
//!
//! `test=slot` drives the lever from the command line for the `slot-ab` case:
//! `slot-do=commit`, `slot-do=stage` (the other slot, as `slot-seq=`) and
//! `slot-do=panic` (before the checks), each ending the boot in a reset.

use crate::efi::{self, Guid, KAI, NV_BS_RT};
use crate::{bootfiles, ipc, log, threads};
use bootslot::{InR, Nv, Record, Slot};
use spin::Mutex;

/// The Boot Loader Interface's vendor GUID, which Limine reads its variables under.
const BLI: Guid = Guid {
    data1: 0x4a67_b082,
    data2: 0x0a4c,
    data3: 0x41cf,
    data4: [0xb6, 0xc7, 0x44, 0x0b, 0x29, 0xbb, 0x8c, 0x4f],
};
const RECORD: &str = "KaiBootState";
const ONE_SHOT: &str = "LoaderEntryOneShot";
/// The bundle entry holding the slot's `kai-slot 1`.
const SLOT_FILE: &str = "kai.slot";
/// How long the required checks may take before the boot counts as failed.
const CHECKS_MS: u64 = 60_000;
/// `last_fault.kind` values.
const FAULT_PANIC: u8 = 1;

/// The record as this boot found it and the slot it runs in, once read: what the
/// fault path writes from.
static BOOTED: Mutex<Option<(Nv, Slot)>> = Mutex::new(None);

/// The BLI entry id of a slot, as `limine.conf` names it.
pub fn entry_id(slot: Slot) -> &'static str {
    match slot {
        Slot::R => "recovery",
        Slot::A => "kai-a",
        Slot::B => "kai-b",
    }
}

/// The value of `key=` on the command line.
fn word(key: &str) -> Option<&'static str> {
    crate::cmdline()
        .split_whitespace()
        .find_map(|word| word.strip_prefix(key)?.strip_prefix('='))
}

/// The slot this kernel was loaded from, or `None` off a slot image.
pub fn this_slot() -> Option<Slot> {
    match word("slot")? {
        "r" => Some(Slot::R),
        "a" => Some(Slot::A),
        "b" => Some(Slot::B),
        _ => None,
    }
}

fn read_record() -> Result<Option<Record>, &'static str> {
    let mut bytes = [0u8; 128];
    match efi::get_variable(&KAI, RECORD, &mut bytes) {
        Ok(len) => Ok(Record::decode(&bytes[..len])),
        Err(efi::Error::NotFound) => Ok(None),
        Err(_) => Err("slots: cannot read KaiBootState"),
    }
}

/// Writes what `after` changed from `before`: the record, then the one-shot.
fn write(before: &Nv, after: &Nv) -> Result<(), efi::Error> {
    if before.record != after.record {
        let mut record = after.record;
        record.boot_seq = before.record.boot_seq.wrapping_add(1);
        efi::set_variable(&KAI, RECORD, NV_BS_RT, &record.encode())?;
    }
    if after.one_shot == before.one_shot {
        return Ok(());
    }
    let result = match after.one_shot {
        Some(slot) => {
            let mut bytes = [0u8; 64];
            let mut n = 0;
            for byte in entry_id(slot).bytes().chain(core::iter::once(0)) {
                bytes[n] = byte;
                n += 2;
            }
            efi::set_variable(&BLI, ONE_SHOT, NV_BS_RT, &bytes[..n])
        }
        None => efi::set_variable(&BLI, ONE_SHOT, NV_BS_RT, &[]),
    };
    match result {
        Err(efi::Error::NotFound) if after.one_shot.is_none() => Ok(()),
        other => other,
    }
}

/// Writes `after` and resets into the slot it arms (or R without a one-shot).
fn finish(before: &Nv, after: &Nv) -> ! {
    if let Err(error) = write(before, after) {
        log!("slots: SetVariable failed: {error:?}");
    }
    log!(
        "slots: next boot {}",
        entry_id(after.one_shot.unwrap_or(Slot::R))
    );
    efi::reset()
}

fn describe(record: &Record, slot: Slot) -> &'static str {
    let state = record.slot(slot);
    match (state.successful, state.tries) {
        (true, _) => "committed",
        (false, 0) => "trial, no tries left",
        (false, _) => "trial",
    }
}

/// This slot's release, from the `kai-slot 1` in its bundle.
fn own_release() -> Option<(u32, u32)> {
    let text = core::str::from_utf8(bootfiles::find(SLOT_FILE)?).ok()?;
    let file = bootslot::slotfile::parse(text)?;
    Some((file.release_seq, file.epoch))
}

/// What a boot in a slot asked for on its command line, in `test=slot`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Restart,
    Commit,
    Stage(u32),
    Panic,
}

fn action() -> Action {
    let seq = word("slot-seq").and_then(|s| s.parse().ok()).unwrap_or(2);
    match word("slot-do") {
        Some("commit") => Action::Commit,
        Some("stage") => Action::Stage(seq),
        Some("panic") => Action::Panic,
        _ => Action::Restart,
    }
}

/// The boot lever, after user space has started. Returns only when the boot is
/// off a slot image, or R stays up, or a slot boot is up and stays running
/// outside `test=slot`.
pub fn on_boot(test: bool) {
    let Some(slot) = this_slot() else {
        return;
    };
    let record = match read_record() {
        Ok(record) => record,
        Err(message) => {
            log!("{message}; booting without the lever");
            return;
        }
    };
    let before = Nv {
        record: record.unwrap_or_default(),
        one_shot: None,
    };
    let mut nv = before;
    log!(
        "slots: booted {} (record {})",
        entry_id(slot),
        if record.is_some() { "read" } else { "missing" }
    );
    if slot == Slot::R {
        return in_r(record.is_some(), before, nv);
    }
    *BOOTED.lock() = Some((before, slot));
    let action = if test { action() } else { Action::Restart };
    if action == Action::Panic {
        panic!("slots: slot-do=panic before the required checks");
    }
    if !checks_pass() {
        crate::fail("slots: the required checks did not pass");
    }
    bootslot::up(&mut nv, slot);
    log!(
        "slots: up in {} ({}, {} tries left)",
        entry_id(slot),
        describe(&nv.record, slot),
        nv.record.slot(slot).tries
    );
    if nv.record.last_fault.kind != 0 {
        log!(
            "slots: the last boot of {} faulted (kind {})",
            entry_id(Slot::ALL[usize::from(nv.record.last_fault.slot).min(2)]),
            nv.record.last_fault.kind
        );
    }
    match action {
        Action::Commit => {
            let epoch = own_release().map_or(0, |(_, epoch)| epoch);
            bootslot::commit(&mut nv, slot, epoch);
            log!("slots: committed {}", entry_id(slot));
        }
        Action::Stage(release_seq) => {
            let Some(other) = slot.other() else {
                crate::fail("slots: R has no other slot");
            };
            bootslot::stage_begin(&mut nv, other);
            bootslot::stage_end(&mut nv, other, release_seq);
            bootslot::restart_into(&mut nv, other);
            log!(
                "slots: staged {} as release {release_seq}; restarting into it",
                entry_id(other)
            );
        }
        Action::Restart | Action::Panic => {}
    }
    if test {
        finish(&before, &nv);
    }
    // A normal boot writes the one-shot that *up* armed and keeps running.
    if let Err(error) = write(&before, &nv) {
        log!("slots: SetVariable failed: {error:?}");
    }
    *BOOTED.lock() = Some((nv, slot));
}

fn in_r(had_record: bool, before: Nv, mut nv: Nv) {
    let decision = if had_record {
        bootslot::r_boots(&mut nv)
    } else {
        let Some((release_seq, _)) = own_release() else {
            log!("slots: no record and no {SLOT_FILE}; R stays up");
            return;
        };
        bootslot::install(&mut nv, release_seq);
        log!(
            "slots: in R with no record: installed release {release_seq} in kai-a and kai-b as trials"
        );
        InR::Arm(Slot::A)
    };
    match decision {
        InR::Arm(slot) => {
            log!(
                "slots: R arms {} ({})",
                entry_id(slot),
                describe(&nv.record, slot)
            );
            finish(&before, &nv)
        }
        InR::Stay => {
            log!("slots: nothing bootable; R stays up");
            if let Err(error) = write(&before, &nv) {
                log!("slots: SetVariable failed: {error:?}");
            }
        }
    }
}

/// ADR 0008 section 5's required checks: the supervisor running and the shell
/// serving, which the shell's endpoint shows, since only the supervisor starts
/// it. `kai-data` mounted and the compositor's frame join later.
fn checks_pass() -> bool {
    for _ in 0..CHECKS_MS / 50 {
        if ipc::server_of("shell").is_some() {
            return true;
        }
        threads::sleep_ms(50);
    }
    false
}

/// From the fault path: records the fault in this boot's slot and deletes the
/// one-shot, so R boots next. Does nothing off a slot image or in R.
pub fn on_fault() {
    let Some((before, slot)) = BOOTED.try_lock().and_then(|mut b| b.take()) else {
        return;
    };
    let mut after = before;
    bootslot::fault(&mut after, slot, FAULT_PANIC, 0);
    match write(&before, &after) {
        Ok(()) => log!("slots: fault recorded in {}; R boots next", entry_id(slot)),
        Err(error) => log!("slots: cannot record the fault: {error:?}"),
    }
    efi::reset()
}
