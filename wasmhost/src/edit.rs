//! `kai:edit@0.1.0` for `lifecycle 2` (P4 task 11): a granted interface through
//! which an editor app reaches the edit service. `send` only queues the
//! message, as `kai:ui/view` queues frames; comphost takes the queue after the
//! call ([`CoreInstance::take_edits`]), sends each message with `EDIT_CALL` and
//! gives the app the answer in a later call, `replies.on-reply`
//! ([`CoreInstance::edit_reply`]). So a call never waits on the edit service,
//! and a version rehearsed beside the active one cannot send an edit twice
//! while it runs: only the shown version's queue is taken. A call that is not
//! wrapped in a transaction cannot send, as it cannot use storage or effects.
//!
//! The interface is linked only for an instance loaded with the `edit` grant
//! ([`Grants`]); without it the import fails to link, and the version does not
//! stage (`Reason::Link`).

use crate::canonical::CoreInstance;
use crate::instance::HostState;
use crate::tables;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use appabi::Val;
use livecore::{Fault, Reason};
use wasmi::{Caller, Error, Extern};

/// The interface whose function this module serves.
pub(crate) const EDIT: &str = "kai:edit/edit";
/// The export the answers go to.
pub(crate) const REPLIES: &str = "kai:edit/replies";

/// Longest message `send` takes: what the edit service's request holds.
pub const MAX_MESSAGE: usize = kabi::MAX_MESSAGE;
/// Most messages waiting for comphost, which takes them after every call it
/// makes into the app; one that would be over this traps the call. (Several
/// events can reach the app between two takes, so this is not a limit per call.)
pub const MAX_QUEUED: usize = 16;

/// A message the app sent, waiting for comphost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditRequest {
    /// What `on-reply` is told the answer is for.
    pub ticket: u32,
    pub message: Vec<u8>,
    /// Whether a person's key or click was being handled when the app sent it
    /// ([`CoreInstance::ui_input`], [`CoreInstance::ui_events_user`]), and not an
    /// answer, an event, the agent's `ui_act` or a timer. comphost sends a build
    /// only for a message that is.
    pub user: bool,
}

/// How many answers deep a person's key still counts as the cause: the key sends
/// a message, its answer makes the app send the next, and so on to this many.
/// An editor sends one message at a time, so an edit typed and then a build asked
/// for is two or three of these; a chain that never ends is cut off here.
pub const MAX_CHAIN: u8 = 16;
/// Most tickets remembered as a person's; the oldest are forgotten first.
const MAX_BY_PERSON: usize = 2 * MAX_QUEUED;

/// The messages not yet taken, and the next ticket.
#[derive(Default)]
pub(crate) struct EditHost {
    pub(crate) queue: Vec<EditRequest>,
    next: u32,
    /// Tickets sent for a person, with how many answers deep each is (0 for the
    /// message their key sent itself).
    by_person: Vec<(u32, u8)>,
    /// How many answers deep the call in progress is, if a person's chain.
    depth: u8,
}

/// `edit.send(message) -> u32`: the message's ticket.
pub(crate) fn send(mut caller: Caller<'_, HostState>, ptr: i32, len: i32) -> Result<i32, Error> {
    let len = len as u32 as usize;
    if len == 0 || len > MAX_MESSAGE {
        return Err(Error::new(alloc::format!(
            "{EDIT} send: {len} bytes; 1 to {MAX_MESSAGE}"
        )));
    }
    let start = ptr as u32 as usize;
    let message = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .and_then(|memory| {
            memory
                .data(&caller)
                .get(start..start.saturating_add(len))
                .map(<[u8]>::to_vec)
        })
        .ok_or_else(|| Error::new("bytes outside memory"))?;
    // Only inside a transaction (an action, a window's input, an event), as for
    // storage and effects: a probation's `init`, `health` or `self-test` cannot
    // send an edit by itself.
    if caller.data().call.is_none() {
        return Err(Error::new(alloc::format!(
            "{EDIT} send: only inside a call that a transaction wraps"
        )));
    }
    let user = caller.data().user_input;
    let host = &mut caller.data_mut().edit;
    if host.queue.len() >= MAX_QUEUED {
        return Err(Error::new(alloc::format!(
            "{EDIT} send: {MAX_QUEUED} messages queued before comphost sent them"
        )));
    }
    let ticket = host.next;
    host.next = host.next.wrapping_add(1);
    if user {
        if host.by_person.len() >= MAX_BY_PERSON {
            host.by_person.remove(0);
        }
        host.by_person.push((ticket, host.depth));
    }
    host.queue.push(EditRequest {
        ticket,
        message,
        user,
    });
    Ok(ticket as i32)
}

impl EditHost {
    /// Forgets `ticket`; if a person's key caused it and the chain is not at
    /// its end, how deep the call that answers it is.
    fn continued(&mut self, ticket: u32) -> Option<u8> {
        let at = self.by_person.iter().position(|(t, _)| *t == ticket)?;
        let (_, depth) = self.by_person.remove(at);
        (depth < MAX_CHAIN).then_some(depth + 1)
    }
}

impl CoreInstance {
    /// The messages the app sent since the last call here, oldest first.
    pub fn take_edits(&mut self) -> Vec<EditRequest> {
        core::mem::take(&mut self.store.data_mut().edit.queue)
    }

    /// Tells the app the answer to `ticket`, or why there is none. An app that
    /// does not export `kai:edit/replies` is told nothing.
    pub fn edit_reply(
        &mut self,
        ticket: u32,
        outcome: Result<Vec<u8>, String>,
    ) -> Result<(), Fault> {
        let Some(export) = self.on_edit_reply.clone() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == REPLIES && f.name == "on-reply")
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:edit/replies table"))?;
        let outcome = match outcome {
            Ok(bytes) => Val::Result(Ok(Some(Box::new(Val::Bytes(bytes))))),
            Err(why) => Val::Result(Err(Some(Box::new(Val::String(why))))),
        };
        // The answer to a message a person's key caused is part of what the key
        // does: the editor sends an edit, and when it is answered the build the
        // person asked for behind it. So what the app sends in that call is the
        // person's too, down to `MAX_CHAIN` answers. An answer to anything else,
        // the agent's `ui_act` included, starts no chain.
        let chain = self.store.data_mut().edit.continued(ticket);
        if let Some(depth) = chain {
            let state = self.store.data_mut();
            state.user_input = true;
            state.edit.depth = depth;
        }
        let result =
            livecore::Instance::call(self, &entry.func(export), &[Val::U32(ticket), outcome]);
        if chain.is_some() {
            let state = self.store.data_mut();
            state.user_input = false;
            state.edit.depth = 0;
        }
        result.map(|_| ())
    }
}
