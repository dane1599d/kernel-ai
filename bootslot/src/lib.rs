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

    /// The committed system slot other than `slot`, if it is bootable.
    fn other_successful(&self, slot: Slot) -> Option<Slot> {
        slot.other().filter(|other| {
            self.slot(*other).successful && self.slot(*other).reason == Reason::None
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
        Some(record)
    }
}

/// The slot the firmware boots: the one-shot, or R without one. Limine erases
/// the one-shot when it reads it.
pub fn boot(nv: &mut Nv) -> Slot {
    nv.one_shot.take().unwrap_or(Slot::R)
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
    let next = if state.reason != Reason::None {
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
    } else if state.tries > 1 {
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

/// Whether booting `slot` keeps the rule: R, a committed slot, or a trial slot
/// with tries left.
pub fn may_boot(record: &Record, slot: Slot) -> bool {
    let state = record.slot(slot);
    slot == Slot::R || (state.reason == Reason::None && (state.successful || state.tries > 0))
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

pub mod drill;

#[cfg(test)]
mod tests;
