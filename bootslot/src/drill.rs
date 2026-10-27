//! Spike S08-1 (`efi-ab`, ADR 0008): one boot of the drill. The kernel runs it
//! on the firmware's real variables and the host runs it on its prediction, so
//! both apply the same events and the host can say where the next boot must land.
//!
//! A boot reads the record, takes one command from the host and ends in a reset,
//! a fault (written by the fault path), a hang, or, in R, by staying up.

use crate::{Nv, Reason, Record, Slot, SlotState};

/// `last_fault.kind` for a panic and for a failed required check.
pub const PANIC: u8 = 1;
pub const CHECK: u8 = 2;
/// The release the install leaves in A.
pub const FIRST_RELEASE: u32 = 1;

/// What the host asks a boot to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// A normal restart after the checks passed.
    Reboot,
    /// Probation passed: commit, then restart.
    Commit,
    /// Stage the other slot (R: a slot that is not committed) and restart into it.
    Stage,
    Rollback,
    Recovery,
    /// Die before anything is written, as a boot that never gets to count.
    PanicEarly,
    /// Checks passed, then a panic.
    PanicLate,
    /// A required check fails.
    FailCheck,
    /// Checks passed, then a hang with interrupts off; the host kills the VM.
    Hang,
    /// Set Limine's `LoaderEntryDefault` to `kai-a`, then recovery on request:
    /// the next boot must still be R.
    SetDefault,
    /// From R when it stays up: restart into `slot`.
    Boot(Slot),
    /// R arms by itself: the host sends this when it expects that.
    None,
}

impl Command {
    pub fn parse(text: &str) -> Option<Command> {
        Some(match text.trim() {
            "reboot" => Command::Reboot,
            "commit" => Command::Commit,
            "stage" => Command::Stage,
            "rollback" => Command::Rollback,
            "recovery" => Command::Recovery,
            "panic-early" => Command::PanicEarly,
            "panic-late" => Command::PanicLate,
            "fail-check" => Command::FailCheck,
            "hang" => Command::Hang,
            "set-default" => Command::SetDefault,
            "boot-a" => Command::Boot(Slot::A),
            "boot-b" => Command::Boot(Slot::B),
            "none" => Command::None,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Command::Reboot => "reboot",
            Command::Commit => "commit",
            Command::Stage => "stage",
            Command::Rollback => "rollback",
            Command::Recovery => "recovery",
            Command::PanicEarly => "panic-early",
            Command::PanicLate => "panic-late",
            Command::FailCheck => "fail-check",
            Command::Hang => "hang",
            Command::SetDefault => "set-default",
            Command::Boot(Slot::A) => "boot-a",
            Command::Boot(_) => "boot-b",
            Command::None => "none",
        }
    }
}

/// How the boot ends once its writes are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Reset,
    /// The fault path runs [`crate::fault`] with this kind, writes, and resets.
    Fault(u8),
    /// Before any write: the kernel panics with nothing armed.
    DieEarly,
    Hang,
    /// R stays up and waits for another command.
    Stay,
    /// Like `Reset`, after writing `LoaderEntryDefault`.
    SetDefault,
}

/// The record a fresh install leaves: A committed and armed, B empty.
pub fn install(nv: &mut Nv) {
    let mut record = Record::default();
    record.slots[Slot::A as usize] = SlotState {
        priority: 1,
        successful: true,
        release_seq: FIRST_RELEASE,
        ..SlotState::default()
    };
    record.slots[Slot::B as usize].reason = Reason::Unbootable;
    record.min_release_seq = FIRST_RELEASE;
    nv.record = record;
    crate::restart_into(nv, Slot::A);
}

/// Stages `slot` with the next release and restarts into it.
fn stage(nv: &mut Nv, slot: Slot) {
    let next = nv
        .record
        .slots
        .iter()
        .map(|s| s.release_seq)
        .max()
        .unwrap_or(0)
        + 1;
    crate::stage_begin(nv, slot);
    crate::stage_end(nv, slot, next);
    crate::restart_into(nv, slot);
}

/// One boot in `slot` (the firmware's pick) taking `command`. `nv` is the state
/// after [`crate::boot`]; a missing or corrupt record is installed first.
pub fn run(nv: &mut Nv, slot: Slot, command: Command) -> End {
    if slot == Slot::R {
        if let crate::InR::Arm(_) = crate::r_boots(nv) {
            return End::Reset;
        }
        return stay(nv, command);
    }
    if command == Command::PanicEarly {
        return End::DieEarly;
    }
    crate::up(nv, slot);
    match command {
        Command::Commit => crate::commit(nv, slot, 1),
        Command::Stage => stage(nv, slot.other().unwrap_or(Slot::B)),
        Command::Rollback => {
            crate::rollback(nv, slot);
        }
        Command::Recovery => crate::recovery(nv),
        Command::SetDefault => {
            crate::recovery(nv);
            return End::SetDefault;
        }
        Command::PanicLate => return End::Fault(PANIC),
        Command::FailCheck => return End::Fault(CHECK),
        Command::Hang => return End::Hang,
        _ => {}
    }
    End::Reset
}

/// R is up and stays: `command` may boot a bootable slot or stage one.
pub fn stay(nv: &mut Nv, command: Command) -> End {
    match command {
        Command::Boot(target) if crate::may_boot(&nv.record, target) => {
            crate::restart_into(nv, target);
            End::Reset
        }
        Command::Stage => {
            let target = [Slot::A, Slot::B]
                .into_iter()
                .find(|s| !nv.record.slot(*s).successful)
                .unwrap_or(Slot::B);
            stage(nv, target);
            End::Reset
        }
        _ => End::Stay,
    }
}

/// Whether two records are the same apart from `boot_seq`, which the kernel
/// writes only together with another change (one write per normal boot).
pub fn same(a: &Record, b: &Record) -> bool {
    Record { boot_seq: 0, ..*a } == Record { boot_seq: 0, ..*b }
}

/// The record as 128 hex digits, for the console.
pub fn hex(record: &Record, out: &mut [u8; 128]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for (n, byte) in record.encode().iter().enumerate() {
        out[2 * n] = DIGITS[usize::from(byte >> 4)];
        out[2 * n + 1] = DIGITS[usize::from(byte & 15)];
    }
}

/// Limine's BLI id for `slot`'s menu entry.
pub fn entry_id(slot: Slot) -> &'static str {
    match slot {
        Slot::R => "recovery",
        Slot::A => "kai-a",
        Slot::B => "kai-b",
    }
}
