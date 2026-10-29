//! The editor app's way to the edit service (P4 task 11): what `kai:edit/edit`
//! `send` queued in the shown version goes to the kernel with `EDIT_CALL`, and the
//! answer comes back to the app through `replies.on-reply`. Only a comphost that
//! was started with the `edit` capability has an `Edit` handle, which is the grant
//! for the one component it serves; any other never takes a queue.
//!
//! A standby version, rehearsed beside the active one during probation, has its
//! queue emptied and dropped: it must not send an edit a second time.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use kabi::handle::{HandleInfo, Kind, MAX_STARTUP};
use livecore::host::{Env, Host};
use user::sys;
use wasmhost::{EditRequest, Grants, LiveInstance};

/// Rounds of sending and answering one pump may make, since an answer can make
/// the app send again.
const ROUNDS: usize = 8;
/// How long one pump may spend with the edit service, so a service that does not
/// answer cannot keep comphost from serving the host's own requests (a swap, a
/// rollback). A message not sent in time is answered with an error.
const BUDGET_MS: u64 = 3_000;

/// What this process is granted: `edit` if the kernel gave it an `Edit` handle.
pub fn grants() -> Grants {
    let mut handles = [HandleInfo::default(); MAX_STARTUP];
    let Ok(count) = sys::startup(&mut handles) else {
        return Grants::default();
    };
    let edit = handles[..count.min(MAX_STARTUP)]
        .iter()
        .any(|h| h.kind() == Some(Kind::Edit));
    Grants { edit }
}

/// One message to the edit service, and its answer or why there is none.
fn call(message: &[u8], reply: &mut [u8]) -> Result<Vec<u8>, String> {
    match sys::edit_call(message, reply) {
        Ok(len) => Ok(reply[..len.min(reply.len())].to_vec()),
        Err(error) => Err(format!("the edit service could not be asked ({error:?})")),
    }
}

/// Sends what the shown version queued and tells it the answers.
pub fn pump<E: Env<Instance = LiveInstance>>(host: &mut Host<E>) {
    let mut reply = vec![0u8; kabi::MAX_MESSAGE];
    let start = sys::time_ns();
    // Set by the first message that could not be sent; the rest of the pump
    // answers with it and sends nothing more.
    let mut failed: Option<String> = None;
    for _ in 0..ROUNDS {
        let mut queued: Vec<EditRequest> = Vec::new();
        host.each_instance(|instance, _, shown| {
            if let LiveInstance::V2(core) = instance {
                let taken = core.take_edits();
                if shown {
                    queued = taken;
                }
            }
        });
        if queued.is_empty() {
            return;
        }
        let answers: Vec<_> = queued
            .iter()
            .map(|request| {
                let spent = sys::time_ns().saturating_sub(start) / 1_000_000;
                if failed.is_none() && spent > BUDGET_MS {
                    failed = Some(String::from("the edit service is too slow"));
                }
                let outcome = match &failed {
                    Some(why) => Err(why.clone()),
                    None => call(&request.message, &mut reply),
                };
                if let (None, Err(why)) = (&failed, &outcome) {
                    failed = Some(why.clone());
                }
                (request.ticket, outcome)
            })
            .collect();
        for (ticket, outcome) in answers {
            // An answer is a call into the app like a window's input: a
            // transaction of its own, so a fault in `on-reply` is the version's,
            // reported and reloaded from its checkpoint, and what the call did
            // is kept only if it did not fault.
            crate::ui::serve_shown(host, |core| core.edit_reply(ticket, outcome));
        }
    }
}
