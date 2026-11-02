//! `kai:models@0.1.0` for `lifecycle 2` (P4 task 10): a granted interface through
//! which the model manager's window reads the model list. As with `kai:system`,
//! `read` only queues; comphost takes the queue after the call
//! ([`CoreInstance::take_models`]), asks the update service for its list, and
//! gives the app the answer in a later call, `listing.on-listing`
//! ([`CoreInstance::models_listing`]). So a call never waits on the system, and a
//! version rehearsed beside the active one cannot read twice while it runs: only
//! the shown version's queue is taken.
//!
//! A read carries nothing but its ticket and changes nothing. The reads wait in
//! the instance's own short queue ([`crate::reads`]), so a list only ever
//! reaches the instance that asked for it. The window cannot switch or import a
//! model through this interface: there is no such function, and `updated`
//! answers the window's process `models` alone.
//!
//! The interface is linked only for an instance loaded with the `models` grant
//! ([`Grants`]); without it the import fails to link, and the version does not
//! stage (`Reason::Link`).

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
pub(crate) const MODELS: &str = "kai:models/models";
/// The export the lists go to.
pub(crate) const LISTING: &str = "kai:models/listing";

pub use crate::reads::MAX_QUEUED;
/// Largest list the app is given, one IPC message; a larger one is replaced by
/// an error.
pub const MAX_REPLY: usize = kabi::MAX_MESSAGE;

/// `models.read() -> u32`: the read's ticket.
pub(crate) fn read(mut caller: Caller<'_, HostState>) -> Result<i32, Error> {
    caller.data_mut().models.read(MODELS)
}

/// Whether `grants` lets an app import `kai:models/models`.
pub(crate) fn granted(grants: Grants) -> bool {
    grants.models
}

impl CoreInstance {
    /// Starts counting a call's reads: where they start in the queue.
    pub(crate) fn models_call_begins(&mut self) {
        self.store.data_mut().models.call_begins();
    }

    /// A call faulted: what it queued is not sent.
    pub(crate) fn models_call_faulted(&mut self) {
        self.store.data_mut().models.call_faulted();
    }

    /// Whether a read waits to be answered.
    pub fn models_waiting(&self) -> bool {
        self.store.data().models.waiting()
    }

    /// The tickets of the reads the app made since the last call here, oldest
    /// first.
    pub fn take_models(&mut self) -> Vec<u32> {
        self.store.data_mut().models.take()
    }

    /// Tells the app the list for `ticket`, or why there is none. A list over
    /// [`MAX_REPLY`] is replaced by an error, and an error text is cut and made
    /// printable. An app that does not export `kai:models/listing` is told
    /// nothing.
    pub fn models_listing(
        &mut self,
        ticket: u32,
        outcome: Result<String, String>,
    ) -> Result<(), Fault> {
        let Some(export) = self.on_models_listing.clone() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == LISTING && f.name == "on-listing")
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:models/listing table"))?;
        let text = |s: String| Some(Box::new(Val::String(s)));
        let outcome = match outcome {
            Ok(list) if list.len() <= MAX_REPLY => Val::Result(Ok(text(list))),
            Ok(_) => Val::Result(Err(text(String::from("the list is too large")))),
            Err(why) => Val::Result(Err(text(error_text(&why)))),
        };
        livecore::Instance::call(self, &entry.func(export), &[Val::U32(ticket), outcome])?;
        Ok(())
    }
}
