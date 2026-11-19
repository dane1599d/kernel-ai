//! `bootslot` (ADR 0008 section 4): which slot boots next, as pure functions over
//! `KaiBootState`, the 64-byte NV+BS+RT EFI variable, and Limine's one-shot
//! (`LoaderEntryOneShot`), which Limine erases when it reads it.
//!
//! The rule: the one-shot always names the slot to boot next if nothing goes
//! wrong, and no one-shot means the recovery slot R. Every event below changes
//! only the record and the one-shot; the kernel writes them with `efi.rs` and
//! resets. The test drives every event sequence up to length 8 and checks that
//! no boot ever lands on a slot with no tries left: each lands on R, a committed
//! slot, or a trial slot with tries.

#![no_std]

pub mod pending;
pub mod slotfile;
pub mod window;

/// The record's magic and version.
pub const MAGIC: &[u8; 4] = b"KAB1";
pub const VERSION: (u8, u8) = (1, 0);
/// The record's size: one small EFI variable.
pub const BYTES: usize = 64;
/// Boots a staged slot gets before it must have committed.
pub const TRIES: u8 = 3;
/// A committed slot's failed boots before R moves to the other slot.
pub const MAX_FAILURES: u8 = 3;

/// A boot slot: recovery, and the two system slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Slot {
    R = 0,
    A = 1,
    B = 2,
}

impl Slot {
    pub const ALL: [Slot; 3] = [Slot::R, Slot::A, Slot::B];

    /// The other system slot; R has none.
    pub fn other(self) -> Option<Slot> {
        match self {
            Slot::A => Some(Slot::B),
            Slot::B => Some(Slot::A),
            Slot::R => None,
        }
    }

    fn from_byte(byte: u8) -> Option<Slot> {
        Slot::ALL.into_iter().find(|slot| *slot as u8 == byte)
    }
}

/// Why a slot is not bootable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    #[default]
    None = 0,
    /// Being written: never boots until staging ends.
    Updating = 1,
    /// A trial slot that spent its tries without committing.
    Unbootable = 2,
}

/// One slot's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotState {
    pub priority: u8,
    /// Boots left for a trial slot.
    pub tries: u8,
    /// Committed: passed probation.
    pub successful: bool,
    pub reason: Reason,
    /// Failed boots of a committed slot in a row.
    pub failures: u8,
    pub release_seq: u32,
}

/// What ended the last boot badly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fault {
    pub slot: u8,
    pub kind: u8,
    pub code: u16,
}

/// `KaiBootState`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub slots: [SlotState; 3],
    /// The slot whose boot was asked for and has not yet finished.
    pub armed: Option<Slot>,
    /// Boot once with every component on its built-in version.
    pub safe_once: bool,
    /// Stay in R once, by request, counting no failure.
    pub recovery_once: bool,
    pub min_release_seq: u32,
    pub min_epoch: u32,
    pub boot_seq: u32,
    pub last_fault: Fault,
    /// The armed trial slot came *up* this boot and spent its try there, so a
    /// fault after it spends none in R. In byte 51, which records before it
    /// left zero and the CRC covers: an older decoder reads the record the same
    /// and an older encoder writes it clear, which only spends the try in R.
    /// The kernel never runs [`boot`] (the firmware picks the slot), so a mark
    /// an earlier boot left can spare one more try in R, once: R clears it
    /// whenever it reads it, so it never loops.
    pub came_up: bool,
}

/// The firmware-held state the events change: the record and the one-shot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Nv {
    pub record: Record,
    pub one_shot: Option<Slot>,
}

/// What R decided when it booted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InR {
    /// Nothing bootable, or recovery was asked for: R stays up.
    Stay,
    /// `slot` is armed to boot next.
    Arm(Slot),
}

impl Record {
    pub fn slot(&self, slot: Slot) -> &SlotState {
        &self.slots[slot as usize]
    }

    fn slot_mut(&mut self, slot: Slot) -> &mut SlotState {
        &mut self.slots[slot as usize]
    }

    /// Whether `slot` holds a release below the floor a commit raised (ADR 0008
    /// §7: "a slot below min_release_seq or min_epoch is refused"). R never
    /// is. The record keeps no epoch for each slot: a release's epoch never
    /// falls as its `release_seq` rises, so a slot below the epoch floor is
    /// below this one too.
    pub fn below_floor(&self, slot: Slot) -> bool {
        slot != Slot::R && self.slot(slot).release_seq < self.min_release_seq
    }

    /// The committed system slot other than `slot`, if it is bootable and not
    /// below the floors.
    fn other_successful(&self, slot: Slot) -> Option<Slot> {
        slot.other().filter(|other| {
            self.slot(*other).successful
                && self.slot(*other).reason == Reason::None
                && !self.below_floor(*other)
        })
    }

    /// The bytes of the record, its CRC-32 last.
    pub fn encode(&self) -> [u8; BYTES] {
        let mut out = [0u8; BYTES];
        out[..4].copy_from_slice(MAGIC);
        out[4] = VERSION.0;
        out[5] = VERSION.1;
        for (n, slot) in self.slots.iter().enumerate() {
            let at = 6 + n * 9;
            out[at] = slot.priority;
            out[at + 1] = slot.tries;
            out[at + 2] = u8::from(slot.successful);
            out[at + 3] = slot.reason as u8;
            out[at + 4] = slot.failures;
            out[at + 5..at + 9].copy_from_slice(&slot.release_seq.to_le_bytes());
        }
        out[33] = self.armed.map_or(0xFF, |slot| slot as u8);
        out[34] = u8::from(self.safe_once) | (u8::from(self.recovery_once) << 1);
        out[35..39].copy_from_slice(&self.min_release_seq.to_le_bytes());
        out[39..43].copy_from_slice(&self.min_epoch.to_le_bytes());
        out[43..47].copy_from_slice(&self.boot_seq.to_le_bytes());
        out[47] = self.last_fault.slot;
        out[48] = self.last_fault.kind;
        out[49..51].copy_from_slice(&self.last_fault.code.to_le_bytes());
        out[51] = u8::from(self.came_up);
        let crc = crc32(&out[..60]);
        out[60..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// The record in `bytes`, or `None` for a bad magic, version, field or CRC:
    /// R then stays up (ADR 0008).
    pub fn decode(bytes: &[u8]) -> Option<Record> {
        if bytes.len() != BYTES || &bytes[..4] != MAGIC || (bytes[4], bytes[5]) != VERSION {
            return None;
        }
        let u32_at = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        if crc32(&bytes[..60]) != u32_at(60) {
            return None;
        }
        let mut record = Record::default();
        for (n, slot) in record.slots.iter_mut().enumerate() {
            let at = 6 + n * 9;
            *slot = SlotState {
                priority: bytes[at],
                tries: bytes[at + 1],
                successful: bytes[at + 2] == 1,
                reason: match bytes[at + 3] {
                    0 => Reason::None,
                    1 => Reason::Updating,
                    2 => Reason::Unbootable,
                    _ => return None,
                },
                failures: bytes[at + 4],
                release_seq: u32_at(at + 5),
            };
        }
        record.armed = match bytes[33] {
            0xFF => None,
            byte => Some(Slot::from_byte(byte)?),
        };
        record.safe_once = bytes[34] & 1 != 0;
        record.recovery_once = bytes[34] & 2 != 0;
        record.min_release_seq = u32_at(35);
        record.min_epoch = u32_at(39);
        record.boot_seq = u32_at(43);
        record.last_fault = Fault {
            slot: bytes[47],
            kind: bytes[48],
            code: u16::from_le_bytes([bytes[49], bytes[50]]),
        };
        record.came_up = bytes[51] & 1 != 0;
        Some(record)
    }
}

/// A `KaiBootState` that is there but does not decode: a bad magic, CRC or
/// field, or a VERSION this code does not know. Not the same as no record: R
/// stays up and writes nothing (ADR 0008), so the floors and commits it held
/// are never rebuilt from nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Undecodable;

/// The record the firmware gave: `found` is the variable's bytes, `None` when
/// it has no `KaiBootState`. `Ok(None)` only when there is none.
pub fn read(found: Option<&[u8]>) -> Result<Option<Record>, Undecodable> {
    match found {
        None => Ok(None),
        Some(bytes) => Record::decode(bytes).map(Some).ok_or(Undecodable),
    }
}

/// What a boot of `slot` does with the record it read (the kernel's lever).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// R with a record: [`r_boots`].
    RBoots(Record),
    /// R with none: [`install`].
    Install,
    /// A system slot with its record: *up*, then probation.
    Slot(Record),
    /// A system slot with no record (NVRAM lost it, the one-shot survived):
    /// nothing is written, so R, booting next, still installs. *Up* on an
    /// empty record would write one with no floors, armed nowhere.
    NoRecord,
}

pub fn start(slot: Slot, record: Option<Record>) -> Start {
    match (slot, record) {
        (Slot::R, Some(record)) => Start::RBoots(record),
        (Slot::R, None) => Start::Install,
        (_, Some(record)) => Start::Slot(record),
        (_, None) => Start::NoRecord,
    }
}

/// The slot the firmware boots: the one-shot, or R without one. Limine erases
/// the one-shot when it reads it.
pub fn boot(nv: &mut Nv) -> Slot {
    let slot = nv.one_shot.take().unwrap_or(Slot::R);
    nv.record.boot_seq = nv.record.boot_seq.wrapping_add(1);
    // A system slot's boot starts again: it has not come up yet. R keeps the
    // mark, which it reads for the boot that ended there.
    if slot != Slot::R {
        nv.record.came_up = false;
    }
    slot
}

/// Asks for `slot` on the next boot, and records that its boot has not finished.
fn arm(nv: &mut Nv, slot: Slot) {
    nv.one_shot = Some(slot);
    nv.record.armed = Some(slot);
}

/// *stage*, before writing `slot`: it never boots until [`stage_end`].
pub fn stage_begin(nv: &mut Nv, slot: Slot) {
    *nv.record.slot_mut(slot) = SlotState {
        reason: Reason::Updating,
        ..SlotState::default()
    };
}

/// *stage*, after the read-back hash: a trial slot above the other.
pub fn stage_end(nv: &mut Nv, slot: Slot, release_seq: u32) {
    let above = slot
        .other()
        .map_or(0, |other| nv.record.slot(other).priority)
        .saturating_add(1);
    *nv.record.slot_mut(slot) = SlotState {
        priority: above,
        tries: TRIES,
        successful: false,
        reason: Reason::None,
        failures: 0,
        release_seq,
    };
}

/// *restart into a slot*.
pub fn restart_into(nv: &mut Nv, slot: Slot) {
    arm(nv, slot);
}

/// *up*: the booted `slot`'s required checks passed.
pub fn up(nv: &mut Nv, slot: Slot) {
    if slot == Slot::R {
        return;
    }
    let state = nv.record.slot_mut(slot);
    if state.successful {
        state.failures = 0;
        nv.record.safe_once = false;
        arm(nv, slot);
    } else {
        state.tries = state.tries.saturating_sub(1);
        if state.tries > 0 {
            arm(nv, slot);
        }
        // This boot spent its try here: a fault from now on spends none in R.
        nv.record.came_up = true;
    }
}

/// *commit*: `slot` passed probation; the floors rise from it, a good slot.
pub fn commit(nv: &mut Nv, slot: Slot, epoch: u32) {
    if slot == Slot::R {
        return;
    }
    let above = slot
        .other()
        .map_or(0, |other| nv.record.slot(other).priority)
        .saturating_add(1);
    let state = nv.record.slot_mut(slot);
    state.successful = true;
    state.tries = 0;
    state.failures = 0;
    state.priority = above;
    let release_seq = state.release_seq;
    nv.record.min_release_seq = nv.record.min_release_seq.max(release_seq);
    nv.record.min_epoch = nv.record.min_epoch.max(epoch);
    // The other slot, if it now holds a release below the floors, never boots
    // again (ADR 0008 §7). One being written keeps its own state.
    if let Some(other) = slot.other()
        && nv.record.below_floor(other)
        && nv.record.slot(other).reason == Reason::None
    {
        let old = nv.record.slot_mut(other);
        old.successful = false;
        old.tries = 0;
        old.priority = 0;
        old.reason = Reason::Unbootable;
    }
    nv.record.came_up = false;
    arm(nv, slot);
}

/// *fault* in `slot`: recorded, and no one-shot, so R boots next.
pub fn fault(nv: &mut Nv, slot: Slot, kind: u8, code: u16) {
    nv.record.last_fault = Fault {
        slot: slot as u8,
        kind,
        code,
    };
    nv.one_shot = None;
}

/// *rollback*: the other committed slot boots next, if there is one.
pub fn rollback(nv: &mut Nv, current: Slot) -> bool {
    match nv.record.other_successful(current) {
        Some(other) => {
            arm(nv, other);
            true
        }
        None => false,
    }
}

/// *recovery on request*.
pub fn recovery(nv: &mut Nv) {
    nv.record.recovery_once = true;
    nv.one_shot = None;
}

/// *R boots*: what to arm, from the slot whose boot did not finish.
pub fn r_boots(nv: &mut Nv) -> InR {
    if nv.record.recovery_once {
        nv.record.recovery_once = false;
        return InR::Stay;
    }
    let Some(slot) = nv.record.armed.filter(|slot| *slot != Slot::R) else {
        return InR::Stay;
    };
    let state = *nv.record.slot(slot);
    // Read once: the boot that ended here came up, or it did not.
    let came_up = core::mem::take(&mut nv.record.came_up);
    let next = if state.reason != Reason::None || nv.record.below_floor(slot) {
        nv.record.other_successful(slot)
    } else if state.successful {
        let failures = state.failures.saturating_add(1);
        nv.record.slot_mut(slot).failures = failures;
        match failures {
            1 => Some(slot),
            2 => {
                nv.record.safe_once = true;
                Some(slot)
            }
            _ => nv.record.other_successful(slot),
        }
    } else if came_up && state.tries > 0 {
        // It came up and spent its try at *up*: the fault after spends none.
        Some(slot)
    } else if !came_up && state.tries > 1 {
        // A boot that never came up spends a try too, or a slot that fails
        // before *up* would loop through R for ever.
        nv.record.slot_mut(slot).tries = state.tries - 1;
        Some(slot)
    } else {
        let gone = nv.record.slot_mut(slot);
        gone.reason = Reason::Unbootable;
        gone.priority = 0;
        nv.record.other_successful(slot)
    };
    match next {
        Some(next) => {
            arm(nv, next);
            InR::Arm(next)
        }
        None => {
            nv.record.armed = None;
            InR::Stay
        }
    }
}

/// *R boots* with no record (a fresh install, or NVRAM cleared): a new record
/// with both system slots as trials of `release_seq`, A above B, and A armed.
/// Nothing is committed until a slot passes probation. R reads `release_seq`
/// from its own `kai-slot 1`; on a fresh install every slot holds the same
/// release. Rebuilding after NVRAM is cleared on a machine whose slots differ
/// reads each slot's record, through `updated` and SLOT_READ.
pub fn install(nv: &mut Nv, release_seq: u32) {
    nv.record = Record::default();
    stage_end(nv, Slot::B, release_seq);
    stage_end(nv, Slot::A, release_seq);
    arm(nv, Slot::A);
}

/// What became of an update staged into `slot` with `release_seq`, seen from a
/// boot running in `running` (`updated`'s reconcile, ADR 0008 §10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trial {
    /// The slot committed that release: the update held.
    Held,
    /// This boot is the slot's trial.
    OnTrial,
    /// Staged, but it has not spent its tries: it may still boot and commit.
    Waiting,
    /// It spent its tries without committing: the update failed.
    Failed,
    /// It never finished staging, or holds another release now: the update
    /// never booted.
    Abandoned,
}

pub fn trial(record: &Record, slot: Slot, release_seq: u32, running: Slot) -> Trial {
    let state = record.slot(slot);
    if state.release_seq != release_seq || state.reason == Reason::Updating {
        Trial::Abandoned
    } else if state.successful {
        Trial::Held
    } else if running == slot {
        Trial::OnTrial
    } else if state.reason == Reason::Unbootable {
        Trial::Failed
    } else {
        Trial::Waiting
    }
}

/// Whether booting `slot` keeps the rule: R, a committed slot, or a trial slot
/// with tries left, never one below the floors.
pub fn may_boot(record: &Record, slot: Slot) -> bool {
    let state = record.slot(slot);
    slot == Slot::R
        || (state.reason == Reason::None
            && (state.successful || state.tries > 0)
            && !record.below_floor(slot))
}

/// The IEEE CRC-32 that GPT uses (as xtask/src/image/gpt.rs).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[cfg(test)]
mod tests;
