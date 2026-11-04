//! `kai:models@0.1.1` for `lifecycle 2` (P4 task 10, #556): a granted interface
//! through which the model manager's window reads the model list and switches
//! the model on a person's key. As with `kai:system`, `read` only queues;
//! comphost takes the queue after the call ([`CoreInstance::take_models`]), asks
//! the update service for its list, and gives the app the answer in a later
//! call, `listing.on-listing` ([`CoreInstance::models_listing`]). So a call never
//! waits on the system, and a version rehearsed beside the active one cannot
//! read twice while it runs: only the shown version's queue is taken.
//!
//! A read carries nothing but its ticket and changes nothing. The reads wait in
//! the instance's own short queue ([`crate::reads`]), so a list only ever
//! reaches the instance that asked for it, and `updated` answers the window's
//! process `models` alone.
//!
//! `use-model` (0.1.1) is the window's one change, held as a Settings change
//! is ([`crate::settings`]): it queues a [`SettingsRequest`] of [`Kind::Change`],
//! `use <sha256>`, which carries `user`, true only when a person's key or click
//! is being handled, and a call may make one. comphost takes the queue
//! ([`CoreInstance::take_model_uses`]) into the same ledger as Settings'
//! requests, which answers a switch without `user` with why not at once; the
//! rest go through `desktop` to the shell, which checks the app and `user`
//! again and runs Kai's `model_use` row, logged and undoable. The answer comes
//! back through `listing.on-used` ([`CoreInstance::models_used`]). An import
//! has no function here: it asks on the trusted path, which a window cannot
//! reach.
//!
//! The interface is linked only for an instance loaded with the `models` grant
//! ([`Grants`]); without it the import fails to link, and the version does not
//! stage (`Reason::Link`).

use crate::canonical::CoreInstance;
use crate::grants::Grants;
use crate::instance::HostState;
use crate::settings::{Kind, SettingsRequest, error_text};
use crate::tables;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use appabi::Val;
use livecore::{Fault, Reason};
use wasmi::{Caller, Error, Extern};

/// The interface whose function this module serves.
pub(crate) const MODELS: &str = "kai:models/models";
/// The export the lists go to.
pub(crate) const LISTING: &str = "kai:models/listing";

pub use crate::reads::MAX_QUEUED;
/// Largest list the app is given, one IPC message; a larger one is replaced by
/// an error.
pub const MAX_REPLY: usize = kabi::MAX_MESSAGE;

/// Most switches waiting for comphost; a call that makes more traps.
pub const MAX_USES: usize = 4;
/// The SHA-256 `use-model` takes, its length, and the word its request starts
/// with: the shell reads the same ones.
pub use kabi::model::{SHA256_HEX, SWITCH, is_sha256_hex};

/// The switches not yet taken, and the next ticket.
#[derive(Default)]
pub(crate) struct Uses {
    queue: Vec<SettingsRequest>,
    next: u32,
    /// Where this call's switches start in the queue, and how many it made.
    call_start: usize,
    in_call: u32,
}

/// `models.read() -> u32`: the read's ticket.
pub(crate) fn read(mut caller: Caller<'_, HostState>) -> Result<i32, Error> {
    caller.data_mut().models.read(MODELS)
}

/// `models.use-model(sha256) -> u32`: the switch's ticket.
pub(crate) fn use_model(
    mut caller: Caller<'_, HostState>,
    ptr: i32,
    len: i32,
) -> Result<i32, Error> {
    let len = len as u32 as usize;
    if len != SHA256_HEX {
        return Err(Error::new(format!(
            "{MODELS} use-model: {len} bytes; a SHA-256 is {SHA256_HEX} hex digits"
        )));
    }
    let start = ptr as u32 as usize;
    let bytes = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .and_then(|memory| {
            memory
                .data(&caller)
                .get(start..start.saturating_add(len))
                .map(<[u8]>::to_vec)
        })
        .ok_or_else(|| Error::new("bytes outside memory"))?;
    let sha256 = String::from_utf8(bytes)
        .ok()
        .filter(|text| is_sha256_hex(text))
        .ok_or_else(|| {
            Error::new(format!(
                "{MODELS} use-model: not {SHA256_HEX} lowercase hex digits"
            ))
        })?;
    // Whether a person's key or click is being handled (`HostState::user_input`).
    let user = caller.data().user_input;
    let uses = &mut caller.data_mut().model_uses;
    if uses.queue.len() >= MAX_USES {
        return Err(Error::new(format!(
            "{MODELS} use-model: {MAX_USES} switches waiting for comphost"
        )));
    }
    // One real key or click authorises one switch.
    if uses.in_call > 0 {
        return Err(Error::new(format!(
            "{MODELS} use-model: a call may make one switch"
        )));
    }
    uses.in_call += 1;
    let ticket = uses.next;
    uses.next = uses
        .next
        .checked_add(1)
        .ok_or_else(|| Error::new(format!("{MODELS} use-model: tickets used up")))?;
    uses.queue.push(SettingsRequest {
        ticket,
        request: format!("{SWITCH}{sha256}"),
        kind: Kind::Change,
        user,
    });
    Ok(ticket as i32)
}

/// Whether `grants` lets an app import `kai:models/models`.
pub(crate) fn granted(grants: Grants) -> bool {
    grants.models
}

impl CoreInstance {
    /// Starts counting a call's reads and switches: where they start in their
    /// queues, and that it has made no switch yet.
    pub(crate) fn models_call_begins(&mut self) {
        let state = self.store.data_mut();
        state.models.call_begins();
        state.model_uses.call_start = state.model_uses.queue.len();
        state.model_uses.in_call = 0;
    }

    /// A call faulted: what it queued is not sent.
    pub(crate) fn models_call_faulted(&mut self) {
        let state = self.store.data_mut();
        state.models.call_faulted();
        let uses = &mut state.model_uses;
        let start = uses.call_start.min(uses.queue.len());
        uses.queue.truncate(start);
    }

    /// The switches the app made since the last call here, oldest first, each
    /// a [`Kind::Change`] request `use <sha256>` carrying whether a person's key
    /// or click made it.
    pub fn take_model_uses(&mut self) -> Vec<SettingsRequest> {
        core::mem::take(&mut self.store.data_mut().model_uses.queue)
    }

    /// Tells the app what came of the switch with `ticket`: the shell's words,
    /// or why it did not run. A text over [`MAX_REPLY`] is replaced by an
    /// error, and an error text is cut and made printable. An app that does not
    /// export `on-used` is told nothing.
    pub fn models_used(
        &mut self,
        ticket: u32,
        outcome: Result<String, String>,
    ) -> Result<(), Fault> {
        let Some(export) = self.on_models_used.clone() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == LISTING && f.name == "on-used")
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:models/listing on-used table"))?;
        let text = |s: String| Some(Box::new(Val::String(s)));
        let outcome = match outcome {
            Ok(words) if words.len() <= MAX_REPLY => Val::Result(Ok(text(words))),
            Ok(_) => Val::Result(Err(text(String::from("the answer is too large")))),
            Err(why) => Val::Result(Err(text(error_text(&why)))),
        };
        livecore::Instance::call(self, &entry.func(export), &[Val::U32(ticket), outcome])?;
        Ok(())
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
