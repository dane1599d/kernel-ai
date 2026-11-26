//! The trusted path's side of the Allow / Deny menu (ADR 0001 D6, ADR 0002 D2a).
//! On Allow it makes request node `request N` for the menu's log entry and mints
//! a `ONCE` handle into the shell's table, whose kernel-held digest names the
//! call exactly as shown (`termcore::once::digest_call`). The shell checks that
//! digest with ONCE_CHECK before the call runs, and a match uses it up. On
//! "Always allow" it also makes the rule's persistent node under `user` and gives
//! the shell a `Grant` handle that can only revoke it, for `rules_remove`; the
//! shell owns the node, so it goes when the shell ends.
//! An Allow on a standing instruction's `rules_add(...)` makes such a node too,
//! noting the one action the instruction may run on its app (P3 task 11), so
//! the user's Allow is the only way a firing's owner finds a rule for it.
//!
//! The shell still holds its run-tier rights, so today the ONCE check is the
//! shell's own; task 14 moves asked calls into `scriptrun`, where the kernel
//! demands the ONCE.

use crate::handle::grant as grants;
use crate::{handle, ipc, process};
use captable::grant::{GrantRef, LABEL, Life, USER};
use kabi::handle::{Flags, Handle, Kind, Rights};
use termcore::menu::{Answer, Granted};

/// What an Allow made: the request node to end once the shell has answered, and
/// what goes on the answer line.
pub struct Allowed {
    pub request: GrantRef,
    pub granted: Granted,
    /// The shell's slot and the `ONCE` handle minted into its table, which
    /// [`end`] closes: told only the badge, the shell cannot.
    shell: usize,
    once: Handle,
}

/// The process slot of the live process `pid`.
fn slot_of(pid: u64) -> Option<usize> {
    process::with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
}

/// Mints what `answer` to menu `id`, which showed `call`, grants the shell.
/// Deny grants nothing.
pub fn allow(id: u64, call: &str, answer: Answer) -> Result<Option<Allowed>, kabi::Error> {
    if answer == Answer::Deny {
        return Ok(None);
    }
    let pid = ipc::server_of("shell").ok_or(kabi::Error::NotFound)?;
    allow_into(pid, id, call, answer).map(Some)
}

/// [`allow`] into the process `pid`: the shell, or a boot case's stand-in for
/// it. What cannot all be granted is refused whole and leaves nothing behind.
pub(crate) fn allow_into(
    pid: u64,
    id: u64,
    call: &str,
    answer: Answer,
) -> Result<Allowed, kabi::Error> {
    debug_assert!(answer != Answer::Deny, "a Deny grants nothing");
    let shell = slot_of(pid).ok_or(kabi::Error::NotFound)?;
    let request = grants::new_request(id)?;
    let digest = termcore::once::digest_call(id, call);
    let once =
        grants::mint_once(shell, Kind::Grant, Rights::empty(), 0, request, digest).and_then(|h| {
            let badge = handle::with(shell, |t| t.table.get(h).map(|e| e.badge))?;
            Ok((h, badge.map_err(kabi::Error::from)?))
        });
    let (once, badge) = match once {
        Ok(minted) => minted,
        Err(error) => {
            let _ = grants::revoke(request);
            return Err(error);
        }
    };
    let note = match answer {
        _ if kabi::event::standing(call).is_some() => Some(Note::Standing),
        Answer::AlwaysKind => Some(Note::Every),
        Answer::AlwaysTarget => Some(Note::Call),
        _ => None,
    };
    let rule = match note
        .map(|note| rule_node(shell, pid, id, call, note))
        .transpose()
    {
        Ok(rule) => rule,
        Err(error) => {
            // Refused whole: the request (and with it the digest's place in
            // the ONCE book) and the ONCE handle go now (bug hunt 3, H3-29).
            let _ = grants::revoke(request);
            close_once(shell, once, request);
            return Err(error);
        }
    };
    Ok(Allowed {
        request,
        granted: Granted {
            once: Some(badge),
            rule,
        },
        shell,
        once,
    })
}

/// What a rule node covers.
enum Note {
    /// `call`'s action, for every target.
    Every,
    /// `call` itself.
    Call,
    /// The standing instruction `call` makes: its action on its app, and the
    /// inverse.
    Standing,
}

/// A persistent `rule <id>` node under `user`, owned by the shell (`pid`), and a
/// handle in the shell's table (`shell`) that can only revoke it. The kernel
/// notes what it covers ([`handle::rules`]), so a call it covers can be checked
/// without a `ONCE`.
fn rule_node(shell: usize, pid: u64, id: u64, call: &str, note: Note) -> Result<u32, kabi::Error> {
    let mut label = [0u8; LABEL];
    let text = alloc::format!("rule from {id}");
    let len = text.len().min(LABEL);
    label[..len].copy_from_slice(&text.as_bytes()[..len]);
    // The shell's, so its exit revokes it: no one else holds a handle that
    // could, and an orphan would keep a place among the kernel's rules until
    // reboot (bug hunt 3, H3-28).
    let node = grants::create(grants::root(USER), pid, Life::Persistent, &label[..len])?;
    let noted = match note {
        Note::Every => handle::rules::record(node, call, true),
        Note::Call => handle::rules::record(node, call, false),
        Note::Standing => handle::rules::record_standing(node, call),
    };
    if let Err(error) = noted {
        let _ = grants::revoke(node);
        return Err(error);
    }
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
/// cannot be used after, matched or not, and its `ONCE` handle is closed.
pub fn end(allowed: &Allowed) {
    // Revoked already when the shell's process ended: nothing more to do.
    let _ = grants::revoke(allowed.request);
    close_once(allowed.shell, allowed.once, allowed.request);
}

/// Closes the `ONCE` handle `once` minted under `request` into `shell`'s table,
/// if it is still there: with its request ended it can do nothing, and the
/// shell, told only its badge, cannot close it (bug hunt 3, H3-30). Matched by
/// its request, so a slot a new process took is left alone.
fn close_once(shell: usize, once: Handle, request: GrantRef) {
    let _ = handle::with(shell, |h| {
        let minted = h
            .table
            .get(once)
            .is_ok_and(|e| e.flags.contains(Flags::ONCE) && e.grant == Some(request));
        if minted {
            let _ = h.table.close(once);
        }
    });
}
