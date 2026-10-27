//! `ctl try <label>`: a swap to `label` rehearsed on copies, so a candidate and
//! its migrations can be checked before anyone asks for the real swap.
//!
//! A fresh instance of the active version takes the last checkpoint. The
//! candidate takes the state from it as a swap would, then passes its self-test
//! and health check and exports. Both copies then run the last requests the host
//! served, and the candidate must not answer `err` where the old version answered
//! `ok`. Last, a fresh old version must read what the candidate writes in the old
//! schema, if it can write it. Nothing goes to the kernel's ledger and the live
//! instance is not asked anything, so a try changes no state. Fetching the
//! candidate counts as a fetch, as it does for a swap.

use crate::host::{Core, Env, Version};
use crate::probation::regressed;
use crate::swap::{Failed, transfer, verify};
use crate::txn::Inputs;
use crate::{Fault, Instance};
use alloc::format;
use alloc::string::String;
use kabi::Error;

/// The reply to `ctl try <label>` while `active` serves.
pub(crate) fn rehearse<E: Env>(
    core: &mut Core<E>,
    active: &Version<E::Instance>,
    label: &str,
) -> String {
    match run(core, active, label) {
        Ok(report) => format!("ok try {label}: {report}"),
        Err(why) => format!("err try {label}: {why}"),
    }
}

fn failed(fault: &Fault) -> String {
    format!("{}: {}", fault.reason.name(), fault.detail)
}

fn run<E: Env>(
    core: &mut Core<E>,
    active: &Version<E::Instance>,
    label: &str,
) -> Result<String, String> {
    if active.label == label {
        return Err(format!("{label} is already active"));
    }
    let artifact = core.env.fetch(label).map_err(|error| match error {
        Error::NotFound => format!("no version {label}"),
        error => format!("cannot fetch {label}: {error:?}"),
    })?;
    let world = active.manifest.world.clone();
    let mut candidate = core
        .stage(label, artifact, Some(&world), true)
        .map_err(|fault| failed(&fault))?;
    let mut old = copy_of(core, active).map_err(|fault| {
        format!(
            "{} could not be copied from its checkpoint: {}",
            active.label,
            failed(&fault)
        )
    })?;
    transfer(&mut old, &mut candidate, &core.checkpoint).map_err(|failure| match failure {
        Failed::Candidate(fault) => failed(&fault),
        Failed::Active(fault) => format!(
            "{} could not write schema {}: {}",
            old.label,
            candidate.schema,
            failed(&fault)
        ),
    })?;
    verify(&mut candidate).map_err(|fault| failed(&fault))?;
    let replayed = replay(core, &mut old, &mut candidate)?;
    // The old copy has run the replay; a fresh one reads the reverse migration.
    drop(old);
    let reverse = reverse(core, active, &mut candidate)?;
    Ok(format!(
        "import ok, self-test ok, health ok, export ok, {replayed}, {reverse}"
    ))
}

/// A new instance of `active` holding the last checkpoint.
fn copy_of<E: Env>(
    core: &mut Core<E>,
    active: &Version<E::Instance>,
) -> Result<Version<E::Instance>, Fault> {
    let instance = core.reload(active, true)?;
    Ok(Version {
        label: active.label.clone(),
        schema: active.schema,
        manifest: active.manifest.clone(),
        artifact: active.artifact.clone(),
        instance,
    })
}

/// Runs the kept requests on both copies. A request the old copy fails ends the
/// replay, since its state is then unknown; one the candidate fails, or answers
/// `err` where the old copy answered `ok`, fails the try.
fn replay<E: Env>(
    core: &Core<E>,
    old: &mut Version<E::Instance>,
    candidate: &mut Version<E::Instance>,
) -> Result<String, String> {
    let total = core.recent.len();
    let (mut done, mut regressions) = (0, 0);
    // Each copy runs from the old version's storage, as on probation; nothing
    // commits.
    let mut storage = core.storage.clone();
    for request in &core.recent {
        let inputs = Inputs::default();
        old.instance.begin(&storage, inputs);
        let before = old.instance.handle(request);
        let old_txn = old.instance.finish();
        let Ok(before) = before else {
            break;
        };
        candidate.instance.begin(&storage, inputs);
        let after = candidate.instance.handle(request);
        let txn = candidate.instance.finish();
        let after = after.map_err(|fault| {
            format!(
                "replaying request {} of {total}: {}",
                done + 1,
                failed(&fault)
            )
        })?;
        done += 1;
        if regressed(&before, &after) || txn != old_txn {
            regressions += 1;
        }
        old_txn.apply(&mut storage);
    }
    if regressions > 0 {
        return Err(format!(
            "{regressions} of {done} replayed requests answered err where {} answered ok",
            old.label
        ));
    }
    if done < total {
        return Ok(format!(
            "{done} of {total} replayed ({} failed the next)",
            old.label
        ));
    }
    Ok(format!("{total} replayed"))
}

/// The candidate's state written in the active version's schema, read by a fresh
/// instance of the active version: what a later swap back would need.
fn reverse<E: Env>(
    core: &mut Core<E>,
    active: &Version<E::Instance>,
    candidate: &mut Version<E::Instance>,
) -> Result<&'static str, String> {
    let schema = active.schema;
    let blob = match candidate.instance.export(schema) {
        Ok(Some(blob)) => blob,
        Ok(None) => return Ok("reverse none"),
        Err(fault) => {
            return Err(format!(
                "writing schema {schema} for {}: {}",
                active.label,
                failed(&fault)
            ));
        }
    };
    let mut fresh = core
        .env
        .load(&active.artifact, true)
        .map_err(|fault| failed(&fault))?;
    match fresh.import(&blob, schema) {
        Ok(0) => Ok("reverse ok"),
        Ok(code) => Err(format!(
            "{} refused the schema {schema} state {} wrote ({code})",
            active.label, candidate.label
        )),
        Err(fault) => Err(format!(
            "{} failed reading the schema {schema} state {} wrote: {}",
            active.label,
            candidate.label,
            failed(&fault)
        )),
    }
}
