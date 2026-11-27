//! The kernel's glue around the steps in the crate root, as
//! `kernel/src/slots.rs` runs it: what it hands [`may_set`](crate::may_set) (the record
//! the firmware holds now, and whether *up*'s record was lost), what *up*
//! keeps when its record is lost, and the order in which a commit takes
//! SETTING and reads the record. It is here, not in the kernel, so the
//! model in `tests/model` drives it and `tests/mutations.rs` undoes each
//! part of it (#1567's review). Each step the glue calls comes in as an
//! argument, so the mutations of those steps still reach the model.

use crate::{Nv, Record, Refused, Slot, UpFailed, commit};

/// [`may_set`](crate::may_set)'s signature: the record the firmware holds now, the record
/// to set, the one-shot, the running slot and whether *up*'s record was
/// lost.
pub type MaySet =
    fn(Option<&Record>, &Record, Option<Slot>, Option<Slot>, bool) -> Result<(), Refused>;

/// Why [`bootrec_set`] did not set the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetFailed<E> {
    /// [`may_set`](crate::may_set) refuses it.
    Refused(Refused),
    /// The record the firmware holds could not be read.
    Unreadable,
    /// A write failed.
    Write(E),
}

/// BOOTREC_SET: `next` and `one_shot` checked by `may` against the record the
/// firmware holds now (`read`, never the record this boot started with) and
/// against `up_lost`, then written by `writes` (`set_writes` in the kernel).
pub fn bootrec_set<E>(
    may: MaySet,
    read: impl FnOnce() -> Result<Option<Record>, ()>,
    up_lost: bool,
    running: Option<Slot>,
    next: &Record,
    one_shot: Option<Slot>,
    writes: impl FnOnce() -> Result<(), E>,
) -> Result<(), SetFailed<E>> {
    let now = read().map_err(|()| SetFailed::Unreadable)?;
    may(now.as_ref(), next, one_shot, running, up_lost).map_err(SetFailed::Refused)?;
    writes().map_err(SetFailed::Write)
}

/// *Up*: `writes` (`up_writes` in the kernel) with `tell`, which tells the
/// fault path *up*'s record (`BOOTED`), and `lost` called when the record
/// was not stored (`UP_LOST`), so [`bootrec_set`] never arms this slot again
/// as a trial this boot.
pub fn up_step<E, T, R, O>(
    writes: impl FnOnce(T, R, O) -> Result<(), UpFailed<E>>,
    tell: T,
    record: R,
    one_shot: O,
    lost: impl FnOnce(),
) -> Result<(), UpFailed<E>> {
    let written = writes(tell, record, one_shot);
    if let Err(UpFailed::Record(_)) = &written {
        lost();
    }
    written
}

/// Probation's commit of `slot`: SETTING held (`hold`) before the record is
/// read (`read`), so no BOOTREC_SET lands between the read and the set and a
/// change `updated` made meanwhile stays; then [`commit`](crate::commit) at `epoch`, set by
/// `set` while SETTING is still held. `None` when there is no record.
pub fn commit_step<G, E>(
    hold: impl FnOnce() -> G,
    read: impl FnOnce() -> Option<Record>,
    slot: Slot,
    epoch: u32,
    set: impl FnOnce(Nv) -> Result<(), E>,
) -> Option<Result<(), E>> {
    let _held = hold();
    let record = read()?;
    let mut nv = Nv {
        record,
        one_shot: Some(slot),
    };
    commit(&mut nv, slot, epoch);
    Some(set(nv))
}
