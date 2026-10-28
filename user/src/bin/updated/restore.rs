//! The way back after a failed update (ADR 0008 §10): before restarting into a
//! new slot, `updated` takes an `update` snapshot of `system` and then writes
//! `/system/updates/pending` naming it, the slot and the release. The file is
//! written after the snapshot, so it is not in it.
//!
//! On every start `updated` reconciles it with the boot record
//! ([`bootslot::trial`]): if the pending slot committed, the update held and
//! the file is cleared; if the slot spent its tries without committing, the
//! update failed, so `system` goes back to the snapshot, which also takes the
//! file away; if the slot never finished staging, the update never booted and
//! the file is cleared. `fs` keeps `/system/trust` and the standing rules
//! through the restore, and `home` is never in the snapshot.

use crate::folders::{self, Folders};
use crate::{record, slot_of};
use alloc::format;
use alloc::string::String;
use bootslot::Trial;
use bootslot::pending::Pending;
use kabi::handle::Handle;
use user::{println, sys};

/// The file in `/system/updates` that names the update in flight.
pub const PENDING: &str = "pending";

/// Records the update in flight, after its snapshot was taken.
pub fn record_pending(updates: Handle, pending: Pending) -> Result<(), String> {
    folders::write(updates, PENDING, format!("{pending}").as_bytes())
}

/// What reconciling found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No `kai-data`, or no update in flight.
    Nothing,
    /// The pending slot is the one running, on trial: wait for its commit.
    OnTrial,
    /// The pending slot is staged but has not spent its tries: wait.
    Waiting,
    /// The pending slot committed; the file is cleared.
    Held,
    /// The pending slot failed before it committed; `system` was restored.
    Restored,
    /// The update never booted, or its snapshot is gone: the file is cleared
    /// and `system` stays as it is.
    Cleared,
}

/// Reconciles the update in flight with the boot record.
pub fn reconcile(handle: Handle) -> Result<Outcome, String> {
    let Some(updates) = Folders::from_startup()?.updates else {
        return Ok(Outcome::Nothing);
    };
    let text = folders::read(updates, PENDING)?.unwrap_or_default();
    let text = core::str::from_utf8(&text).unwrap_or("");
    if text.trim().is_empty() {
        return Ok(Outcome::Nothing);
    }
    let Some(pending) = Pending::parse(text) else {
        println!("updated: {PENDING} holds {text:?}; cleared");
        folders::write(updates, PENDING, b"")?;
        return Ok(Outcome::Cleared);
    };
    let record = record(handle)?;
    let running = slot_of(
        sys::slot_info(handle, 1)
            .map_err(|e| format!("SLOT_INFO: {e:?}"))?
            .active,
    )?;
    let (release, slot) = (pending.release_seq, pending.slot);
    match bootslot::trial(&record, slot, release, running) {
        Trial::OnTrial => Ok(Outcome::OnTrial),
        Trial::Waiting => Ok(Outcome::Waiting),
        Trial::Held => {
            folders::write(updates, PENDING, b"")?;
            println!("updated: release {release} in {slot:?} committed; the update held");
            Ok(Outcome::Held)
        }
        Trial::Abandoned => {
            folders::write(updates, PENDING, b"")?;
            println!("updated: release {release} in {slot:?} never booted; {PENDING} cleared");
            Ok(Outcome::Cleared)
        }
        Trial::Failed => restore(updates, pending),
    }
}

fn restore(updates: Handle, pending: Pending) -> Result<Outcome, String> {
    let (release, slot, id) = (pending.release_seq, pending.slot, pending.snapshot);
    match folders::update_restore(updates, id)? {
        folders::Restore::Done => {}
        // Repair dropped it, or it was never committed: there is nothing to go
        // back to, and keeping the file would refuse every later update.
        folders::Restore::Gone => {
            println!(
                "updated: release {release} in {slot:?} failed, but update snapshot {id} is gone; {PENDING} cleared"
            );
            folders::write(updates, PENDING, b"")?;
            return Ok(Outcome::Cleared);
        }
    }
    println!(
        "updated: release {release} in {slot:?} failed before it committed; restored the system volume from update snapshot {id}"
    );
    let left = folders::read(updates, PENDING)?.is_some_and(|t| !t.is_empty());
    println!(
        "updated: {PENDING} is {} after the restore",
        if left { "still there" } else { "gone" }
    );
    Ok(Outcome::Restored)
}
