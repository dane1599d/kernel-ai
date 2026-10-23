//! `ctl swap`: stage a candidate, move the state into it, check it, then put it on
//! probation (or commit it at once when probation is off). Every attempt that gets
//! as far as the kernel's BEGIN ends in a COMMIT or a ROLLBACK naming the step that
//! failed; the active version is only ever asked to export.

use crate::host::{Core, Env, Exit, Mode, Probation, Report, Version, export_own, text};
use crate::{Fault, Instance, Reason};
use alloc::boxed::Box;
use alloc::format;
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{flag, kind};

type Step<I> = Result<(Mode<I>, Vec<u8>), Exit>;

pub(crate) fn swap<E: Env>(
    core: &mut Core<E>,
    mut active: Version<E::Instance>,
    label: &str,
    force: bool,
) -> Step<E::Instance> {
    if active.label == label {
        return Ok((
            Mode::Serving(active),
            text(format!("err {label} is already active")),
        ));
    }
    let artifact = match core.env.fetch(label) {
        Ok(artifact) => artifact,
        Err(Error::NotFound) => {
            return Ok((
                Mode::Serving(active),
                text(format!("err no version {label}")),
            ));
        }
        Err(error) => {
            return Ok((
                Mode::Serving(active),
                text(format!("err cannot fetch {label}: {error:?}")),
            ));
        }
    };
    // The update point: the state now, which the kernel keeps until the swap ends.
    let blob = match export_own(&mut active) {
        Ok(blob) => blob,
        Err(fault) => {
            core.reload_after(&mut active, &fault, false)?;
            let reply = format!(
                "err cannot swap: {} {} failed to export ({}: {})",
                core.name,
                active.label,
                fault.reason.name(),
                fault.detail
            );
            return Ok((Mode::Serving(active), text(reply)));
        }
    };
    let begin = Report {
        label,
        schema: u64::from(active.schema),
        flags: if force { flag::FORCE } else { 0 },
        blob: &blob,
        ..Report::new(kind::BEGIN)
    };
    match core.env.report(&begin) {
        Ok(()) => {}
        Err(Error::Permission) => {
            let reply = format!("err {label} failed before; use ctl swap {label} force");
            return Ok((Mode::Serving(active), text(reply)));
        }
        // The store evicted it between the fetch and the begin.
        Err(Error::NotFound) => {
            return Ok((
                Mode::Serving(active),
                text(format!("err no version {label}")),
            ));
        }
        Err(error) => {
            return Err(Exit {
                code: crate::host::EXIT_REFUSED,
                why: format!("the kernel refused a begin event: {error:?}"),
            });
        }
    }
    core.checkpoint = blob.clone();
    let world = active.manifest.world.clone();
    let mut candidate = match core.stage(label, artifact, Some(&world), false) {
        Ok(candidate) => candidate,
        Err(fault) => return refused(core, active, label, &fault, 0),
    };
    match transfer(&mut active, &mut candidate, &blob) {
        Ok(()) => {}
        Err(Failed::Candidate(fault)) => return refused(core, active, label, &fault, 0),
        Err(Failed::Active(fault)) => {
            // The old version failed while writing an older schema: not the
            // candidate's doing. Close the swap without blame, and reload it.
            let why = Fault::new(
                Reason::Export,
                format!(
                    "{} failed to write schema {}: {}",
                    active.label, candidate.schema, fault.detail
                ),
            );
            let step = refused(core, active, label, &why, flag::POISON)?;
            let Mode::Serving(mut active) = step.0 else {
                unreachable!("refused keeps the active version")
            };
            core.reload_after(&mut active, &fault, false)?;
            return Ok((Mode::Serving(active), step.1));
        }
    }
    let candidate_blob = match verify(&mut candidate) {
        Ok(blob) => blob,
        Err(fault) => return refused(core, active, label, &fault, 0),
    };
    let settings = &core.settings;
    if settings.probation_calls == 0 && settings.probation_ms == 0 {
        commit(core, &candidate, &candidate_blob, 0, 0)?;
        return Ok((
            Mode::Serving(candidate),
            text(format!("ok committed {label}")),
        ));
    }
    let calls = settings.probation_calls;
    active.instance.quiet(true);
    let probation = Probation {
        standby: active,
        candidate,
        began_ms: core.env.now_ms(),
        served: 0,
        regressions: 0,
        health_ok: 0,
    };
    Ok((
        Mode::Probation(Box::new(probation)),
        text(format!("ok probation {label} ({calls} calls)")),
    ))
}

/// Ends a swap that failed before probation; the active version carries on.
fn refused<E: Env>(
    core: &mut Core<E>,
    active: Version<E::Instance>,
    label: &str,
    fault: &Fault,
    flags: u64,
) -> Step<E::Instance> {
    core.report(&Report {
        reason: Some(fault.reason),
        label,
        detail: &fault.detail,
        flags,
        ..Report::new(kind::ROLLBACK)
    })?;
    let reply = match fault.detail.is_empty() {
        true => format!("err rolled back {label}: {}", fault.reason.name()),
        false => format!(
            "err rolled back {label}: {}: {}",
            fault.reason.name(),
            fault.detail
        ),
    };
    Ok((Mode::Serving(active), text(reply)))
}

/// Moves the state into the candidate: its own migration from the active schema,
/// or, if it reads only an older schema, the active version's export in that one.
/// Which side of a transfer failed.
pub(crate) enum Failed {
    Candidate(Fault),
    /// The active version, while writing an older schema for the candidate.
    Active(Fault),
}

pub(crate) fn transfer<I: Instance>(
    active: &mut Version<I>,
    candidate: &mut Version<I>,
    blob: &[u8],
) -> Result<(), Failed> {
    let migrate = |fault: Fault| Failed::Candidate(Fault::new(Reason::Migrate, fault.detail));
    let refused = |schema: u32, code: u32, label: &str| {
        Failed::Candidate(Fault::new(
            Reason::Migrate,
            format!("{label} refused schema {schema} state ({code})"),
        ))
    };
    let (from, to) = (active.schema, candidate.schema);
    match candidate.instance.import(blob, from).map_err(migrate)? {
        0 => return Ok(()),
        1 => {}
        code => return Err(refused(from, code, &candidate.label)),
    }
    let no_migration = || {
        Failed::Candidate(Fault::new(
            Reason::NoMigration,
            format!(
                "{} cannot read schema {from} and {} cannot write schema {to}",
                candidate.label, active.label
            ),
        ))
    };
    if to >= from {
        return Err(no_migration());
    }
    let older = active
        .instance
        .export(to)
        .map_err(Failed::Active)?
        .ok_or_else(no_migration)?;
    match candidate.instance.import(&older, to).map_err(migrate)? {
        0 => Ok(()),
        code => Err(refused(to, code, &candidate.label)),
    }
}

/// The candidate's own checks, then its state in its own schema.
pub(crate) fn verify<I: Instance>(candidate: &mut Version<I>) -> Result<Vec<u8>, Fault> {
    match candidate.instance.self_test()? {
        0 => {}
        code => {
            return Err(Fault::new(
                Reason::SelfTest,
                format!("its self-test returned {code}"),
            ));
        }
    }
    match candidate.instance.health()? {
        0 => {}
        code => {
            return Err(Fault::new(
                Reason::Health,
                format!("its health check returned {code}"),
            ));
        }
    }
    export_own(candidate)
}

/// The candidate becomes the active version, with `blob` as its checkpoint.
pub(crate) fn commit<E: Env>(
    core: &mut Core<E>,
    candidate: &Version<E::Instance>,
    blob: &[u8],
    served: u64,
    regressions: u64,
) -> Result<(), Exit> {
    core.report(&Report {
        label: &candidate.label,
        schema: u64::from(candidate.schema),
        served,
        regressions,
        blob,
        ..Report::new(kind::COMMIT)
    })?;
    core.checkpoint = blob.to_vec();
    Ok(())
}
