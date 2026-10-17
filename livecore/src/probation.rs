//! Probation by running both: each request goes to the old version (the standby)
//! first, whose state is what the kernel keeps, then to the candidate, whose reply
//! is sent. A candidate fault therefore rolls back losing nothing, and the caller
//! gets the standby's reply. Components have no side effects, so running a request
//! twice cannot be told from running it once.
//!
//! The candidate is committed when it has served `probation_calls` requests, spent
//! `probation_ms` on probation, and passes a last health check. With probation_ms
//! set, a candidate that sees no requests is also committed after four times that
//! long and two passing health checks. Time never rolls anything back.

use crate::host::{Core, Env, Exit, Mode, Probation, Report, Stop, Version, export_own, text};
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
    let standby_reply = run_standby(core, &mut probation.standby, request)?;
    match probation.candidate.instance.handle(request) {
        Ok(reply) => {
            probation.served += 1;
            let regressed = reply.starts_with(b"err")
                && standby_reply.as_ref().is_some_and(|r| r.starts_with(b"ok"));
            if regressed {
                probation.regressions += 1;
            }
            Ok((end_if_due(core, probation)?, reply))
        }
        Err(fault) => {
            // Both failed: the request is to blame, not the candidate.
            let flags = if standby_reply.is_none() {
                flag::POISON
            } else {
                0
            };
            let (mode, _) = roll_back(core, probation, &fault, flags)?;
            let reply = standby_reply
                .unwrap_or_else(|| text(format!("err {} failed on this request", core.name)));
            Ok((mode, reply))
        }
    }
}

/// Runs a request on the standby and checkpoints its state. A standby fault
/// reloads it from the last checkpoint; there is then no standby reply.
fn run_standby<E: Env>(
    core: &mut Core<E>,
    standby: &mut Version<E::Instance>,
    request: &[u8],
) -> Result<Option<Vec<u8>>, Exit> {
    let result = standby
        .instance
        .handle(request)
        .map_err(Stop::Fault)
        .and_then(|reply| core.checkpoint(standby).map(|()| reply));
    match result {
        Ok(reply) => Ok(Some(reply)),
        Err(Stop::Exit(exit)) => Err(exit),
        Err(Stop::Fault(fault)) => {
            core.reload_after(standby, &fault, true)?;
            Ok(None)
        }
    }
}

/// No request came: check the candidate, and commit it if its time is up.
pub(crate) fn tick<E: Env>(
    core: &mut Core<E>,
    mut probation: Probation<E::Instance>,
) -> Result<Mode<E::Instance>, Exit> {
    match health(&mut probation.candidate) {
        Ok(()) => {
            probation.health_ok += 1;
            end_if_due(core, probation)
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

/// Commits the candidate if probation is over and it passes a last check.
fn end_if_due<E: Env>(
    core: &mut Core<E>,
    mut probation: Probation<E::Instance>,
) -> Result<Mode<E::Instance>, Exit> {
    let settings = &core.settings;
    let elapsed = core.env.now_ms().saturating_sub(probation.began_ms);
    let served = probation.served >= settings.probation_calls && elapsed >= settings.probation_ms;
    let idle = settings.probation_ms > 0
        && elapsed >= settings.probation_ms.saturating_mul(4)
        && probation.health_ok >= 2;
    if !served && !idle {
        return Ok(Mode::Probation(Box::new(probation)));
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
            Ok(Mode::Serving(probation.candidate))
        }
        Err(fault) => roll_back(core, probation, &fault, 0).map(|(mode, _)| mode),
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
