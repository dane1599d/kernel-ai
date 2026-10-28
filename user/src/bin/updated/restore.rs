//! The way back after a failed update (ADR 0008 §10): before restarting into a
//! new slot, `updated` takes an `update` snapshot of `system` and then writes
//! `/system/updates/pending` naming it, the slot and the release. The file is
//! written after the snapshot, so it is not in it.
//!
//! On every start `updated` reconciles: if the pending slot has committed, the
//! update held and the file is cleared; if this boot runs in another slot and
//! the pending one never committed, the new slot failed before it could, so
//! `system` goes back to the snapshot, which also takes the file away. `home` is
//! never in the snapshot and is never rolled back.

use crate::folders::{self, Folders};
use crate::{record, slot_of};
use alloc::format;
use alloc::string::String;
use bootslot::Slot;
use kabi::handle::Handle;
use user::{println, sys};

/// The file in `/system/updates` that names the update in flight.
pub const PENDING: &str = "pending";

/// What `pending` holds: `snapshot <id> slot <A|B> release <seq>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    pub snapshot: u64,
    pub slot: Slot,
    pub release_seq: u32,
}

impl Pending {
    pub fn encode(&self) -> String {
        let slot = if self.slot == Slot::A { "A" } else { "B" };
        format!(
            "snapshot {} slot {slot} release {}\n",
            self.snapshot, self.release_seq
        )
    }

    pub fn parse(text: &str) -> Option<Pending> {
        let mut words = text.split_whitespace();
        let mut field = |key| (words.next() == Some(key)).then(|| words.next()).flatten();
        let snapshot = field("snapshot")?.parse().ok()?;
        let slot = match field("slot")? {
            "A" => Slot::A,
            "B" => Slot::B,
            _ => return None,
        };
        let release_seq = field("release")?.parse().ok()?;
        Some(Pending {
            snapshot,
            slot,
            release_seq,
        })
    }
}

/// Records the update in flight, after its snapshot was taken.
pub fn record_pending(updates: Handle, pending: Pending) -> Result<(), String> {
    folders::write(updates, PENDING, pending.encode().as_bytes())
}

/// What reconciling found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No `kai-data`, or no update in flight.
    Nothing,
    /// The pending slot is the one running, on trial: wait for its commit.
    OnTrial,
    /// The pending slot committed; the file is cleared.
    Held,
    /// The pending slot failed before it committed; `system` was restored.
    Restored,
}

/// Reconciles the update in flight with the boot record.
pub fn reconcile(handle: Handle) -> Result<Outcome, String> {
    let Some(updates) = Folders::from_startup().updates else {
        return Ok(Outcome::Nothing);
    };
    let text = folders::read(updates, PENDING)?.unwrap_or_default();
    let text = core::str::from_utf8(&text).unwrap_or("");
    if text.trim().is_empty() {
        return Ok(Outcome::Nothing);
    }
    let pending = Pending::parse(text).ok_or_else(|| format!("{PENDING} holds {text:?}"))?;
    let record = record(handle)?;
    let running = slot_of(
        sys::slot_info(handle, 1)
            .map_err(|e| format!("SLOT_INFO: {e:?}"))?
            .active,
    )?;
    let state = record.slot(pending.slot);
    if state.successful && state.release_seq == pending.release_seq {
        folders::write(updates, PENDING, b"")?;
        println!(
            "updated: release {} in {:?} committed; the update held",
            pending.release_seq, pending.slot
        );
        return Ok(Outcome::Held);
    }
    if running == pending.slot {
        return Ok(Outcome::OnTrial);
    }
    folders::update_restore(updates, pending.snapshot)?;
    println!(
        "updated: release {} in {:?} failed before it committed; restored the system volume from update snapshot {}",
        pending.release_seq, pending.slot, pending.snapshot
    );
    let left = folders::read(updates, PENDING)?.is_some_and(|t| !t.is_empty());
    println!(
        "updated: {PENDING} is {} after the restore",
        if left { "still there" } else { "gone" }
    );
    Ok(Outcome::Restored)
}
