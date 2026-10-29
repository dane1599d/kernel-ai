//! Calls into the serving version that are not requests: a window's input, a
//! close, an event from the OS. [`Host::serve_with`] runs one as a transaction,
//! the way [`Host::handle`] serves a request: it reads the storage as it was when
//! the call began, its writes commit with the version's checkpoint, and its
//! effects come back with their tickets for whoever runs them. Without it such a
//! call has no storage or effects: they trap (`wasmhost/src/effects.rs`).
//!
//! What differs from a request: there is no reply, and no `kai-undo` entry is
//! recorded. A window's edits are the user's own, not the agent's changes, and
//! one entry a keystroke would push every change `undo` can reach out of its
//! history of 16.

use crate::host::{Env, Exit, Host, Mode, Stop, Version, check_txn};
use crate::txn::Txn;
use crate::{Fault, Instance};
use alloc::string::String;

/// What a transaction did besides its result: the effects it queued and the
/// ticket the first one got. Tickets run on across calls, so one never repeats.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Performed {
    pub txn: Txn,
    pub first_ticket: u32,
}

impl Performed {
    /// The effects as the `effect <ticket> <WAVE>` lines of a reply.
    pub fn effect_lines(&self) -> String {
        self.txn.effect_lines(self.first_ticket)
    }

    /// Whether the call queued no effects.
    pub fn is_empty(&self) -> bool {
        self.txn.effects.is_empty()
    }
}

/// A call's result and what it did, if it ran.
type Done<R> = Option<(R, Performed)>;

impl<E: Env> Host<E> {
    /// Runs `f` on the serving version as one transaction, and commits what it
    /// did. `Ok(None)` if the version faulted (it was reported and reloaded from
    /// its checkpoint, and nothing `f` did is kept) or no version is serving.
    ///
    /// A suspended app wakes first. During probation `f` runs on the candidate,
    /// the version shown, with no transaction, as before: it can read and draw
    /// but not use storage or effects, and its result has no [`Performed`] worth
    /// running.
    pub fn serve_with<R>(
        &mut self,
        f: impl FnOnce(&mut E::Instance) -> Result<R, Fault>,
    ) -> Result<Done<R>, Exit> {
        let mode = core::mem::replace(&mut self.mode, Mode::Moving);
        let (mode, out) = match mode {
            Mode::Serving(version) => self.serve_call(version, f)?,
            Mode::Suspended(_) => match self.resume(mode) {
                (Mode::Serving(version), _) => self.serve_call(version, f)?,
                (mode, _) => (mode, None),
            },
            Mode::Probation(mut probation) => {
                let out = f(&mut probation.candidate.instance)
                    .ok()
                    .map(|r| (r, Performed::default()));
                (Mode::Probation(probation), out)
            }
            other => (other, None),
        };
        self.mode = mode;
        Ok(out)
    }

    fn serve_call<R>(
        &mut self,
        mut version: Version<E::Instance>,
        f: impl FnOnce(&mut E::Instance) -> Result<R, Fault>,
    ) -> Result<(Mode<E::Instance>, Done<R>), Exit> {
        let inputs = self.core.inputs();
        let storage = self.core.storage.clone();
        version.instance.begin(&storage, inputs);
        let result = f(&mut version.instance);
        let txn = version.instance.finish();
        let done = result
            .and_then(|r| check_txn(&txn, inputs, 0).map(|()| r))
            .map_err(Stop::Fault)
            .and_then(|r| {
                self.core.commit_as(&mut version, &txn, false)?;
                self.core.next_ticket = inputs.first_ticket.wrapping_add(txn.effects.len() as u32);
                Ok(r)
            });
        match done {
            Ok(r) => {
                let performed = Performed {
                    txn,
                    first_ticket: inputs.first_ticket,
                };
                Ok((Mode::Serving(version), Some((r, performed))))
            }
            Err(Stop::Exit(exit)) => Err(exit),
            Err(Stop::Fault(fault)) => {
                self.core.reload_after(&mut version, &fault, false)?;
                Ok((Mode::Serving(version), None))
            }
        }
    }
}
