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
use bootslot::{InR, Nv, Reason, Record, Slot};
use core::sync::atomic::{AtomicBool, Ordering};
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
    write_one_shot(after.one_shot)
}

/// Sets Limine's one-shot to `slot`'s entry, or deletes it.
fn write_one_shot(one_shot: Option<Slot>) -> Result<(), efi::Error> {
    let result = match one_shot {
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
        Err(efi::Error::NotFound) if one_shot.is_none() => Ok(()),
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
    lever(test);
    LEVER_DONE.store(true, Ordering::SeqCst);
}

/// Whether this boot's lever has written its record: until then `updated`'s
/// BOOTREC_* and SLOT_WRITE calls answer Busy, so the lever's write never
/// overwrites a change of theirs.
pub fn lever_done() -> bool {
    LEVER_DONE.load(Ordering::SeqCst)
}

static LEVER_DONE: AtomicBool = AtomicBool::new(false);

fn lever(test: bool) {
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
    if let Some(other) = slot.other()
        && nv.record.slot(other).reason != Reason::None
    {
        log!(
            "slots: {} cannot boot ({:?})",
            entry_id(other),
            nv.record.slot(other).reason
        );
    }
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
    let trial = !nv.record.slot(slot).successful;
    *BOOTED.lock() = Some((nv, slot));
    // A trial slot runs on probation until it commits (ADR 0008 §5).
    if trial && let Err(why) = threads::spawn("probation", probation, slot as usize) {
        log!(
            "slots: no probation thread ({why}); {} stays a trial",
            entry_id(slot)
        );
    }
}

/// ADR 0008 §5's probation: `calls` requests served by the shell and `ms` with
/// no fault, then a last check; an idle machine commits after 4 times `ms` and 2
/// passing checks. A fault takes the fault path, which never commits.
struct Probation {
    calls: u64,
    ms: u64,
}

impl Probation {
    /// The ADR's figures, or `probation_calls=` and `probation_ms=` for tests:
    /// an installed system's command line names only its slot.
    fn from_cmdline() -> Probation {
        let number = |key, default| word(key).and_then(|v| v.parse().ok()).unwrap_or(default);
        Probation {
            calls: number("probation_calls", PROBATION_CALLS),
            ms: number("probation_ms", PROBATION_MS),
        }
    }
}

/// ADR 0008 §5: 20 requests and 10 minutes.
const PROBATION_CALLS: u64 = 20;
const PROBATION_MS: u64 = 600_000;
/// How often probation looks.
const PROBATION_TICK_MS: u64 = 1_000;

fn served() -> u64 {
    let (kernel, processes) = ipc::replies("shell");
    kernel + processes
}

fn probation(raw: usize) {
    let Some(slot) = Slot::ALL.get(raw).copied() else {
        return;
    };
    let rules = Probation::from_cmdline();
    let start = crate::hpet::nanos() / 1_000_000;
    let first = served();
    let mut idle_checks = 0;
    loop {
        threads::sleep_ms(PROBATION_TICK_MS);
        let elapsed = crate::hpet::nanos() / 1_000_000 - start;
        let calls = served().saturating_sub(first);
        let busy = calls >= rules.calls && elapsed >= rules.ms;
        // The idle rule: a check at 2 and at 4 times `ms`.
        if calls < rules.calls && elapsed >= 2 * rules.ms * (idle_checks + 1) && idle_checks < 2 {
            if !checks_pass() {
                crate::fail("slots: a required check failed on probation");
            }
            idle_checks += 1;
        }
        if busy || idle_checks == 2 {
            if !checks_pass() {
                crate::fail("slots: the last check before commit failed");
            }
            return commit(slot, calls, elapsed);
        }
    }
}

/// *commit*: from the record as the firmware holds it now, so a change
/// `updated` made since boot stays.
fn commit(slot: Slot, calls: u64, elapsed_ms: u64) {
    let Ok(Some(record)) = read_record() else {
        log!("slots: no record to commit {} in", entry_id(slot));
        return;
    };
    let mut nv = Nv {
        record,
        one_shot: Some(slot),
    };
    let epoch = own_release().map_or(0, |(_, epoch)| epoch);
    bootslot::commit(&mut nv, slot, epoch);
    match set(nv.record, nv.one_shot) {
        Ok(()) => log!(
            "slots: {} passed probation ({calls} requests, {} s) and is committed",
            entry_id(slot),
            elapsed_ms / 1000
        ),
        Err(error) => log!("slots: cannot commit {}: {error:?}", entry_id(slot)),
    }
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

/// `test=update`: the boot comes up as a normal one, then `updated step=check`
/// stages the other slot and restarts into it. Fails if it has not by then.
pub fn update_case() -> ! {
    on_boot(false);
    threads::sleep_ms(UPDATE_CASE_MS);
    crate::fail("update: updated did not restart into the staged slot");
}

/// `test=probation`: a trial boot comes up, the kernel sends the shell
/// `probation-lines=` requests (none for the idle rule), and the slot must
/// commit within a minute of probation's end.
pub fn probation_case() -> ! {
    on_boot(false);
    let Some(slot) = this_slot() else {
        crate::fail("probation: not on a slot disk");
    };
    let lines: u64 = word("probation-lines")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for _ in 0..lines {
        if ipc::call("shell", b"1 + 1", 30_000).is_err() {
            crate::fail("probation: the shell did not answer");
        }
    }
    log!("probation: sent the shell {lines} requests");
    let wait = Probation::from_cmdline().ms * 4 + 60_000;
    for _ in 0..wait / PROBATION_TICK_MS {
        if matches!(read_record(), Ok(Some(record)) if record.slot(slot).successful) {
            log!("probation: {} is committed", entry_id(slot));
            crate::test_exit(crate::qemu::ExitCode::Success);
        }
        threads::sleep_ms(PROBATION_TICK_MS);
    }
    crate::fail("probation: the slot never committed");
}

/// How long `update_case` waits for `updated` to stage and restart.
const UPDATE_CASE_MS: u64 = 240_000;

/// `KaiBootState` as the firmware holds it, for BOOTREC_GET.
pub fn record() -> Result<Option<Record>, &'static str> {
    read_record()
}

/// Whether this boot's slot is committed, for SLOT_WRITE's rule on R.
pub fn active_committed() -> bool {
    match (this_slot(), read_record()) {
        (Some(slot), Ok(Some(record))) => record.slot(slot).successful,
        _ => false,
    }
}

/// BOOTREC_SET: writes `record` as given and the one-shot for `one_shot`, and
/// keeps them as what the fault path writes from.
pub fn set(record: Record, one_shot: Option<Slot>) -> Result<(), efi::Error> {
    efi::set_variable(&KAI, RECORD, NV_BS_RT, &record.encode())?;
    write_one_shot(one_shot)?;
    let mut booted = BOOTED.lock();
    if let Some((_, slot)) = *booted {
        *booted = Some((Nv { record, one_shot }, slot));
    }
    Ok(())
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
