//! `kai:system@0.1.0` for `lifecycle 2` (P4 task 9): a granted interface through
//! which the System monitor reads the machine. As with `kai:settings`, `read`
//! only queues; comphost takes the queue after the call
//! ([`CoreInstance::take_system`]), reads the kernel's `METRICS` and asks llm for
//! its `health`, and gives the app the sample in a later call,
//! `samples.on-sample` ([`CoreInstance::system_sample`]). So a call never waits
//! on the system, and a version rehearsed beside the active one cannot read twice
//! while it runs: only the shown version's queue is taken.
//!
//! A read carries nothing but its ticket, so there is no grammar to check, and a
//! read is allowed at any time (it changes nothing). The queue is short
//! ([`MAX_QUEUED`]) because the app wants the latest sample, not a backlog. A call
//! that faults leaves nothing queued.
//!
//! The interface is linked only for an instance loaded with the `system` grant
//! ([`Grants`]); without it the import fails to link, and the version does not
//! stage (`Reason::Link`).
//!
//! Tickets count from 0 for each instance. comphost must therefore drop the reads
//! it still holds when it starts a new instance (a nap and resume, or a swap),
//! or an old read's answer would reach a new instance as the answer to its own
//! first read.

use crate::canonical::CoreInstance;
use crate::grants::Grants;
use crate::instance::HostState;
use crate::settings::error_text;
use crate::tables;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use appabi::Val;
use livecore::{Fault, Reason};
use wasmi::{Caller, Error};

/// The interface whose function this module serves.
pub(crate) const SYSTEM: &str = "kai:system/system";
/// The export the samples go to.
pub(crate) const SAMPLES: &str = "kai:system/samples";

/// Most reads waiting for comphost; a call that makes more traps.
pub const MAX_QUEUED: usize = 4;
/// Largest sample the app is given; a larger one is replaced by an error.
pub const MAX_REPLY: usize = systemabi::MAX_BYTES;

/// The tickets of the reads not yet taken, and the next ticket.
#[derive(Default)]
pub(crate) struct SystemHost {
    queue: Vec<u32>,
    next: u32,
    /// Where this call's reads start in the queue.
    call_start: usize,
}

/// `system.read() -> u32`: the read's ticket.
pub(crate) fn read(mut caller: Caller<'_, HostState>) -> Result<i32, Error> {
    let host = &mut caller.data_mut().system;
    if host.queue.len() >= MAX_QUEUED {
        return Err(Error::new(alloc::format!(
            "{SYSTEM} read: {MAX_QUEUED} reads waiting for comphost"
        )));
    }
    let ticket = host.next;
    host.next = host
        .next
        .checked_add(1)
        .ok_or_else(|| Error::new(alloc::format!("{SYSTEM} read: tickets used up")))?;
    host.queue.push(ticket);
    Ok(ticket as i32)
}

/// Whether `grants` lets an app import `kai:system/system`.
pub(crate) fn granted(grants: Grants) -> bool {
    grants.system
}

impl CoreInstance {
    /// Starts counting a call's reads: where they start in the queue.
    pub(crate) fn system_call_begins(&mut self) {
        let host = &mut self.store.data_mut().system;
        host.call_start = host.queue.len();
    }

    /// A call faulted: what it queued is not sent.
    pub(crate) fn system_call_faulted(&mut self) {
        let host = &mut self.store.data_mut().system;
        let start = host.call_start.min(host.queue.len());
        host.queue.truncate(start);
    }

    /// The tickets of the reads the app made since the last call here, oldest
    /// first.
    pub fn take_system(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.store.data_mut().system.queue)
    }

    /// Tells the app the sample for `ticket`, or why there is none. A sample over
    /// [`MAX_REPLY`] is replaced by an error, and an error text is cut and made
    /// printable. An app that does not export `kai:system/samples` is told
    /// nothing.
    pub fn system_sample(
        &mut self,
        ticket: u32,
        outcome: Result<Vec<u8>, String>,
    ) -> Result<(), Fault> {
        let Some(export) = self.on_system_sample.clone() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == SAMPLES && f.name == "on-sample")
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:system/samples table"))?;
        let text = |s: String| Some(Box::new(Val::String(s)));
        let outcome = match outcome {
            Ok(bytes) if bytes.len() <= MAX_REPLY => {
                Val::Result(Ok(Some(Box::new(Val::Bytes(bytes)))))
            }
            Ok(_) => Val::Result(Err(text(String::from("the sample is too large")))),
            Err(why) => Val::Result(Err(text(error_text(&why)))),
        };
        livecore::Instance::call(self, &entry.func(export), &[Val::U32(ticket), outcome])?;
        Ok(())
    }
}
