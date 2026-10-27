//! Slots and the boot record (lane U, ADR 0008 §10).
//! - SLOT_INFO, SLOT_READ, SLOT_WRITE and SLOT_FLUSH each need an `Update`
//!   handle, which only the `updated` service holds, and reach only the slot
//!   partitions R, A and B, found by type GUID: the kernel checks every range
//!   against the slot, so the ESP and `kai-data` can never be named.
//! - SLOT_WRITE refuses the slot this boot runs in, and R unless that slot is
//!   committed, so a running system never overwrites itself or its way back.
//! - BOOTREC_GET and BOOTREC_SET read and write `KaiBootState` and Limine's
//!   one-shot through `slots.rs`; a one-shot naming a slot the record says may
//!   not boot is refused.

use super::fs::{flush_in, on, read_in, write_in};
use crate::block::Device;
use crate::handle::{self, grant::Kernel};
use crate::process::uaccess::{copy_from_user, copy_to_user};
use crate::slots;
use bootslot::{Record, Slot};
use kabi::handle::{Handle, Kind, Rights};
use kabi::update::{self, MAX_IO, RECORD_BYTES, SECTOR};
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::SLOT_INFO,
    call::SLOT_READ,
    call::SLOT_WRITE,
    call::SLOT_FLUSH,
    call::BOOTREC_GET,
    call::BOOTREC_SET,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::SLOT_INFO => info(slot, a[0], a[1], a[2]),
        call::SLOT_READ => {
            checked(slot, a[0], Rights::READ)?;
            read_in(slot, kind_of(a[1])?, a[2], a[3], a[4])
        }
        call::SLOT_WRITE => {
            checked(slot, a[0], Rights::WRITE)?;
            let target = slot_of(a[1])?;
            if !writable(target) {
                return Err(Error::Permission);
            }
            write_in(slot, kind_of(a[1])?, a[2], a[3], a[4])
        }
        call::SLOT_FLUSH => {
            checked(slot, a[0], Rights::WRITE)?;
            flush_in(kind_of(a[1])?)
        }
        call::BOOTREC_GET => get(slot, a[0], a[1]),
        _ => set(slot, a[0], a[1], a[2], a[3]),
    })
}

/// Checks that `raw` is an `Update` handle with `rights` naming the system.
fn checked(slot: usize, raw: u64, rights: Rights) -> Result<(), Error> {
    let handle = Handle(u32::try_from(raw).map_err(|_| Error::BadHandle)?);
    let entry = handle::with(slot, |h| {
        h.table.check(handle, Kind::Update, rights, &Kernel)
    })?
    .map_err(Error::from)?;
    match entry.object {
        update::SYSTEM => Ok(()),
        _ => Err(Error::NotFound),
    }
}

fn slot_of(raw: u64) -> Result<Slot, Error> {
    match raw {
        update::R => Ok(Slot::R),
        update::A => Ok(Slot::A),
        update::B => Ok(Slot::B),
        _ => Err(Error::Invalid),
    }
}

fn kind_of(raw: u64) -> Result<diskmap::Guid, Error> {
    Ok(match slot_of(raw)? {
        Slot::R => diskmap::KAI_R_TYPE,
        Slot::A => diskmap::KAI_A_TYPE,
        Slot::B => diskmap::KAI_B_TYPE,
    })
}

/// SLOT_WRITE's rule: never the active slot; R only from a committed slot.
/// Off a slot disk there is no active slot, and R is not written.
fn writable(target: Slot) -> bool {
    match (slots::this_slot(), target) {
        (None, _) => false,
        (Some(active), target) if active == target => false,
        (Some(_), Slot::R) => slots::active_committed(),
        (Some(_), _) => true,
    }
}

fn info(slot: usize, raw: u64, which: u64, out: u64) -> Result<u64, Error> {
    checked(slot, raw, Rights::INSPECT)?;
    let target = slot_of(which)?;
    let sectors = on(kind_of(which)?, |disk: &mut dyn Device| disk.sectors())?;
    let active = slots::this_slot().map_or(update::NONE, |s| s as u64);
    let flags = if writable(target) {
        update::WRITABLE
    } else {
        0
    };
    let mut bytes = [0u8; core::mem::size_of::<update::SlotInfo>()];
    for (field, value) in
        bytes
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .zip([sectors, MAX_IO / SECTOR, active, flags])
    {
        *field = value.to_le_bytes();
    }
    copy_to_user(slot, out, &bytes)?;
    Ok(0)
}

fn get(slot: usize, raw: u64, buf: u64) -> Result<u64, Error> {
    checked(slot, raw, Rights::READ)?;
    let record = slots::record()
        .map_err(|_| Error::Unavailable)?
        .ok_or(Error::NotFound)?;
    copy_to_user(slot, buf, &record.encode())?;
    Ok(RECORD_BYTES)
}

fn set(slot: usize, raw: u64, buf: u64, one_shot: u64, restart: u64) -> Result<u64, Error> {
    checked(slot, raw, Rights::WRITE)?;
    let mut bytes = [0u8; RECORD_BYTES as usize];
    copy_from_user(slot, buf, &mut bytes)?;
    let record = Record::decode(&bytes).ok_or(Error::Invalid)?;
    let one_shot = match one_shot {
        update::NONE => None,
        raw => Some(slot_of(raw)?),
    };
    if one_shot.is_some_and(|next| !bootslot::may_boot(&record, next)) {
        return Err(Error::Invalid);
    }
    if restart > 1 {
        return Err(Error::Invalid);
    }
    slots::set(record, one_shot).map_err(|_| Error::Unavailable)?;
    if restart == 1 {
        crate::log!(
            "update: restarting into {}",
            slots::entry_id(one_shot.unwrap_or(Slot::R))
        );
        crate::efi::reset();
    }
    Ok(0)
}
