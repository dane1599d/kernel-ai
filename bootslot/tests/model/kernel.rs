//! The kernel's steps the walk drives, as `bootslot` gives them to
//! `kernel/src/slots.rs`: [`REAL`] is those functions, and `tests/mutations.rs`
//! swaps one for the code before a fix to see the walk fail.

use bootslot::glue::SetFailed;
use bootslot::{ArmFailed, Nv, Record, Refused, Slot, Told, UpFailed};

/// A step's result in the walk, whose writes fail with `()`.
pub type Done = Result<(), ()>;
pub type Step<'a> = &'a mut dyn FnMut() -> Done;
pub type Tell<'a> = &'a mut dyn FnMut();
pub type TellSet<'a> = &'a mut dyn FnMut(Told);
pub type OneShot<'a> = &'a mut dyn FnMut(Option<Slot>) -> Done;
pub type UpDone = Result<(), UpFailed<()>>;
pub type ArmDone = Result<(), ArmFailed<()>>;
pub type MaySet =
    fn(Option<&Record>, &Record, Option<Slot>, Option<Slot>, bool) -> Result<(), Refused>;

#[derive(Clone, Copy)]
pub struct Kernel {
    /// [`bootslot::up_writes`]: tell, the record, the one-shot.
    pub up_writes: fn(Tell, Step, Step) -> UpDone,
    /// [`bootslot::set_writes`]: tell, the record, the one-shot.
    pub set_writes: fn(TellSet, Step, Step) -> Done,
    /// [`bootslot::fault_writes`]: the record, the one-shot.
    pub fault_writes: fn(Step, Step) -> Done,
    /// [`bootslot::arm_writes`]: armed, next, the one-shot, the record.
    pub arm_writes: fn(Option<Slot>, Slot, OneShot, Step) -> ArmDone,
    /// [`bootslot::may_set`]: BOOTREC_SET's refusals.
    pub may_set: MaySet,
    /// [`bootslot::rebase`]: `updated`'s second try after a Busy.
    pub rebase: fn(&mut Record, &Record, Slot) -> bool,
    /// [`bootslot::glue::bootrec_set`]: what BOOTREC_SET hands `may_set`.
    pub bootrec_set: BootrecSet,
    /// [`bootslot::glue::up_step`]: *up*'s writes, and the lost record kept.
    pub up_step: UpStep,
    /// [`bootslot::glue::commit_step`]: SETTING, then the read, then the set.
    pub commit_step: CommitStep,
}

pub type BootrecSet = fn(MaySet, Read, bool, Option<Slot>, &Record, Option<Slot>, Step) -> SetDone;
pub type UpStep = fn(fn(Tell, Step, Step) -> UpDone, Tell, Step, Step, Tell) -> UpDone;
pub type CommitStep = fn(Tell, ReadNow, Slot, u32, SetNv) -> Option<Done>;
pub type Read<'a> = &'a mut dyn FnMut() -> Result<Option<Record>, ()>;
pub type ReadNow<'a> = &'a mut dyn FnMut() -> Option<Record>;
pub type SetNv<'a> = &'a mut dyn FnMut(Nv) -> Done;
pub type SetDone = Result<(), SetFailed<()>>;

pub const REAL: Kernel = Kernel {
    up_writes: |tell, record, one_shot| bootslot::up_writes(tell, record, one_shot),
    set_writes: |tell, record, one_shot| bootslot::set_writes(tell, record, one_shot),
    fault_writes: |record, one_shot| bootslot::fault_writes(record, one_shot),
    arm_writes: |armed, next, one_shot, record| bootslot::arm_writes(armed, next, one_shot, record),
    may_set: bootslot::may_set,
    rebase: bootslot::rebase,
    bootrec_set: |may, read, lost, running, next, one_shot, writes| {
        bootslot::glue::bootrec_set(may, read, lost, running, next, one_shot, writes)
    },
    up_step: |writes, tell, record, one_shot, lost| {
        bootslot::glue::up_step(writes, tell, record, one_shot, lost)
    },
    commit_step: |hold, read, slot, epoch, set| {
        bootslot::glue::commit_step(hold, read, slot, epoch, set)
    },
};
