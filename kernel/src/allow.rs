//! The trusted path's side of the Allow / Deny menu (ADR 0001 D6, ADR 0002 D2a).
//! On Allow it makes request node `request N` for the menu's log entry and mints
//! a `ONCE` handle into the shell's table, whose kernel-held digest names the
//! question exactly as shown (`termcore::once::digest`). The shell checks that
//! digest with ONCE_CHECK before the call runs, and a match uses it up. On
//! "Always allow" it also makes the rule's persistent node under `user` and gives
//! the shell a `Grant` handle that can only revoke it, for `rules_remove`.
//!
//! The shell still holds its run-tier rights, so today the ONCE check is the
//! shell's own; task 14 moves asked calls into `scriptrun`, where the kernel
//! demands the ONCE.

use crate::handle::grant as grants;
use crate::{handle, ipc, process};
use captable::grant::{GrantRef, LABEL, Life, USER};
use kabi::handle::{Kind, Rights};
use termcore::menu::{Answer, Granted};

/// What an Allow made: the request node to end once the shell has answered, and
/// what goes on the answer line.
pub struct Allowed {
    pub request: GrantRef,
    pub granted: Granted,
}

/// The process slot of the live process `pid`.
fn slot_of(pid: u64) -> Option<usize> {
    process::with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
}

/// Mints what `answer` to menu `id`, which showed `question`, grants the shell.
/// Deny grants nothing.
pub fn allow(id: u64, question: &str, answer: Answer) -> Result<Option<Allowed>, kabi::Error> {
    if answer == Answer::Deny {
        return Ok(None);
    }
    let shell = ipc::server_of("shell")
        .and_then(slot_of)
        .ok_or(kabi::Error::NotFound)?;
    let request = grants::new_request(id)?;
    let digest = termcore::once::digest(id, question);
    let once =
        grants::mint_once(shell, Kind::Grant, Rights::empty(), 0, request, digest).and_then(|h| {
            handle::with(shell, |t| t.table.get(h).map(|e| e.badge))?.map_err(kabi::Error::from)
        });
    let once = match once {
        Ok(badge) => badge,
        Err(error) => {
            let _ = grants::revoke(request);
            return Err(error);
        }
    };
    let rule = match answer {
        Answer::AlwaysKind | Answer::AlwaysTarget => Some(rule_node(shell, id)?),
        _ => None,
    };
    Ok(Some(Allowed {
        request,
        granted: Granted {
            once: Some(once),
            rule,
        },
    }))
}

/// A persistent `rule <id>` node under `user`, and a handle in the shell's table
/// that can only revoke it.
fn rule_node(shell: usize, id: u64) -> Result<u32, kabi::Error> {
    let mut label = [0u8; LABEL];
    let text = alloc::format!("rule from {id}");
    let len = text.len().min(LABEL);
    label[..len].copy_from_slice(&text.as_bytes()[..len]);
    let node = grants::create(grants::root(USER), 0, Life::Persistent, &label[..len])?;
    let entry = captable::Entry {
        grant: Some(node),
        ..captable::Entry::new(Kind::Grant, Rights::REVOKE, node.pack())
    };
    match grants::insert(shell, entry) {
        Ok(handle) => Ok(handle.0),
        Err(error) => {
            let _ = grants::revoke(node);
            Err(error)
        }
    }
}

/// Ends the request an Allow made, once the shell has answered: its `ONCE` digest
/// cannot be used after, matched or not.
pub fn end(allowed: &Allowed) {
    // Revoked already when the shell's process ended: nothing more to do.
    let _ = grants::revoke(allowed.request);
}
