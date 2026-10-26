//! Probation by running both: each request goes to the old version (the standby)
//! first, whose state is what the kernel keeps, then to the candidate, whose reply
//! is sent. A candidate fault therefore rolls back losing nothing, and the caller
//! gets the standby's reply.
//!
//! Each request is one transaction on each version, both from the same storage,
//! clock and seed (ADR 0003 section 4). The storage writes that commit are the
//! standby's, with its checkpoint; the effects that go out are those of the
//! version whose reply is sent, so an effect is performed once whichever version
//! answers. Different writes or effects count as a regression, as replies do.
//!
//! The candidate is committed when it has served `probation_calls` requests, spent
//! `probation_ms` on probation, and passes a last health check. With probation_ms
//! set, a candidate that sees no requests is also committed after four times that
//! long and two passing health checks. Time never rolls anything back.

use crate::call::REPLY_HEADER;
use crate::host::{Core, Env, Exit, Mode, Probation, Report, Stop, Version, export_own, text};
use crate::txn::{Inputs, Txn};
use crate::{Fault, Instance, Reason};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::{flag, kind};

type Step<I> = Result<(Mode<I>, Vec<u8>), Exit>;

pub(crate) fn dual_run<E: Env>(
    core: &mut Core<E>,
    mut probation: Probation<E::Instance>,
    request: &[u8],
) -> Step<E::Instance> {
    let inputs = core.inputs();
    let storage = core.storage.clone();
    let (standby_reply, standby_txn, undo) =
        match run_standby(core, &mut probation.standby, request, inputs) {
            Ok(done) => done,
            Err(Stop::Exit(exit)) => return Err(exit),
            Err(Stop::Fault(fault)) => return standby_failed(core, probation, &fault),
        };
    // The candidate sees the storage the standby saw, not the standby's writes.
    match core.run(&mut probation.candidate, request, inputs, &storage) {
        Ok((reply, txn)) => {
            probation.served += 1;
            if regressed(&standby_reply, &reply) || txn != standby_txn {
                probation.regressions += 1;
            }
            let (mode, rolled_back) = end_if_due(core, probation)?;
            // A candidate that failed its last check does not give the answer.
            let (reply, txn) = if rolled_back {
                (standby_reply, standby_txn)
            } else {
                (reply, txn)
            };
            Ok((mode, core.performed(reply, &txn, inputs, undo)))
        }
        Err(fault) => {
            let (mode, _) = roll_back(core, probation, &fault, 0)?;
            Ok((
                mode,
                core.performed(standby_reply, &standby_txn, inputs, undo),
            ))
        }
    }
}

/// Whether the new version failed where the old one did not: `err` where it
/// answered `ok`, or a `kai-reply` refused or `err(...)` where it gave a result.
pub(crate) fn regressed(old: &[u8], new: &[u8]) -> bool {
    let failed = |reply: &[u8]| {
        let Some(rest) = reply
            .strip_prefix(REPLY_HEADER.as_bytes())
            .and_then(|rest| rest.strip_prefix(b"\n"))
        else {
            return reply.starts_with(b"err");
        };
        rest.starts_with(b"refused ") || rest.starts_with(b"result err(")
    };
    let succeeded =
        |reply: &[u8]| reply.starts_with(b"ok") || reply.starts_with(REPLY_HEADER.as_bytes());
    failed(new) && succeeded(old) && !failed(old)
}

/// Runs a request on the standby, commits its writes with its checkpoint, and
/// keeps the request for `ctl try`.
fn run_standby<E: Env>(
    core: &mut Core<E>,
    standby: &mut Version<E::Instance>,
    request: &[u8],
    inputs: Inputs,
) -> Result<(Vec<u8>, Txn, Option<u64>), Stop> {
    let storage = core.storage.clone();
    let (reply, txn) = core.run(standby, request, inputs, &storage)?;
    let undo = core.commit(standby, &txn)?;
    core.remember(request);
    Ok((reply, txn, undo))
}

/// The old version failed on a request. The kernel still holds its state from
/// before the request, so the old version is reloaded from that and the candidate
/// is not given the request: both stay in step with what the kernel holds, and no
/// write can be acknowledged that a rollback would lose.
fn standby_failed<E: Env>(
    core: &mut Core<E>,
    mut probation: Probation<E::Instance>,
    fault: &Fault,
) -> Step<E::Instance> {
    if let Err(exit) = core.reload_after(&mut probation.standby, fault, true) {
        // Giving up on the old version: close the swap first, without blaming the
        // candidate for what the old version did.
        let _ = roll_back(core, probation, fault, flag::POISON);
        return Err(exit);
    }
    let reply = format!(
        "err {} failed on this request: {}",
        core.name,
        fault.reason.name()
    );
    Ok((Mode::Probation(Box::new(probation)), text(reply)))
}

/// No request came: check the candidate, and commit it if its time is up.
pub(crate) fn tick<E: Env>(
    core: &mut Core<E>,
    mut probation: Probation<E::Instance>,
) -> Result<Mode<E::Instance>, Exit> {
    match health(&mut probation.candidate) {
        Ok(()) => {
            probation.health_ok += 1;
            end_if_due(core, probation).map(|(mode, _)| mode)
        }
        Err(fault) => roll_back(core, probation, &fault, 0).map(|(mode, _)| mode),
    }
}

fn health<I: Instance>(version: &mut Version<I>) -> Result<(), Fault> {
    match version.instance.health()? {
        0 => Ok(()),
        code => Err(Fault::new(
            Reason::Health,
            format!("its health check returned {code}"),
        )),
    }
}

/// Commits the candidate if probation is over and it passes a last check; true
/// when that check failed and the candidate was rolled back.
fn end_if_due<E: Env>(
    core: &mut Core<E>,
    mut probation: Probation<E::Instance>,
) -> Result<(Mode<E::Instance>, bool), Exit> {
    let settings = &core.settings;
    let elapsed = core.env.now_ms().saturating_sub(probation.began_ms);
    let served = probation.served >= settings.probation_calls && elapsed >= settings.probation_ms;
    let idle = settings.probation_ms > 0
        && elapsed >= settings.probation_ms.saturating_mul(4)
        && probation.health_ok >= 2;
    if !served && !idle {
        return Ok((Mode::Probation(Box::new(probation)), false));
    }
    let checked =
        health(&mut probation.candidate).and_then(|()| export_own(&mut probation.candidate));
    match checked {
        Ok(blob) => {
            crate::swap::commit(
                core,
                &probation.candidate,
                &blob,
                probation.served,
                probation.regressions,
            )?;
            Ok((Mode::Serving(probation.candidate), false))
        }
        Err(fault) => roll_back(core, probation, &fault, 0).map(|(mode, _)| (mode, true)),
    }
}

/// Drops the candidate; the standby, whose state the kernel holds, carries on.
/// Returns the new mode and the candidate's label.
pub(crate) fn roll_back<E: Env>(
    core: &mut Core<E>,
    mut probation: Probation<E::Instance>,
    fault: &Fault,
    flags: u64,
) -> Result<(Mode<E::Instance>, String), Exit> {
    let label = probation.candidate.label.clone();
    core.report(&Report {
        reason: Some(fault.reason),
        label: &label,
        served: probation.served,
        regressions: probation.regressions,
        flags,
        detail: &fault.detail,
        ..Report::new(kind::ROLLBACK)
    })?;
    probation.standby.instance.quiet(false);
    Ok((Mode::Serving(probation.standby), label))
}
