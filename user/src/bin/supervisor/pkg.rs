//! The supervisor's `run` and `withdraw`: `pkg` starts a `start=pkg` app on a
//! package version, and takes one back out (`supervise::pkgrun`).

use super::{Service, now_ms, save, try_save};
use alloc::string::String;
use alloc::vec::Vec;
use supervise::ServiceSpec;
use user::{println, sys};

/// `run <name> version=<label>` from `caller`: a `start=pkg` app on the package
/// version the store holder added, and only from that holder
/// (`supervise::check_run`). A running app keeps its version: swapping one is
/// comphost's, under probation.
pub(super) fn run_package(services: &mut [Service], run: supervise::Run, caller: u64) -> String {
    let held: Vec<(&ServiceSpec, Option<u64>)> =
        services.iter().map(|s| (&s.spec, s.pid)).collect();
    let index = match supervise::check_run(run, &held, caller) {
        Ok(index) => index,
        Err(why) => {
            println!("{:?} from pid {caller} refused: {why}", run.encode());
            return String::from(why);
        }
    };
    let service = &mut services[index];
    let name = service.spec.name.clone();
    if service.pid.is_some() {
        return match service.record.version.as_deref() == Some(run.version) {
            true => String::from("ok"),
            false => String::from(supervise::RUNS_ANOTHER),
        };
    }
    service.record.version = Some(String::from(run.version));
    service.record.stopped = false;
    service.record.gave_up = false;
    service.start_at_ms = Some(now_ms());
    println!("{name} starts on package version {} for pkg", run.version);
    save(services);
    String::from("ok")
}

/// `withdraw <name> version=<label>` from `caller`: the store holder took that
/// package version back out (an undo), so the `start=pkg` app ends if it runs
/// it and waits for `pkg` to name a version again, its end counted as no crash
/// (`withdrawn`). Only from that holder (`supervise::check_run`); an app on
/// another version keeps running. Until the app's exit is seen its pid stays
/// set, so a `run` arriving in that gap is RUNS_ANOTHER, which `pkg` may retry.
pub(super) fn withdraw_package(
    services: &mut [Service],
    withdraw: supervise::Run,
    caller: u64,
) -> String {
    let held: Vec<(&ServiceSpec, Option<u64>)> =
        services.iter().map(|s| (&s.spec, s.pid)).collect();
    let index = match supervise::check_run(withdraw, &held, caller) {
        Ok(index) => index,
        Err(why) => {
            println!(
                "{:?} from pid {caller} refused: {why}",
                withdraw.encode_withdraw()
            );
            return String::from(why);
        }
    };
    let service = &mut services[index];
    if service.record.version.as_deref() != Some(withdraw.version) {
        return String::from("ok");
    }
    let name = service.spec.name.clone();
    // Saved cleared before the app is ended: a supervisor that dies in
    // between must not restart the withdrawn version. A failed save puts the
    // version back and ends nothing; a failed kill puts it back and
    // saves it again.
    let pid = service.pid;
    service.withdrawn = pid.is_some();
    let withdrawn = supervise::withdraw_saving_first(
        &mut *services,
        |services| services[index].record.version.take(),
        |services, version| services[index].record.version = version,
        try_save,
        |services| match services[index].pid {
            Some(pid) => sys::kill(pid).map(drop),
            None => Ok(()),
        },
    );
    let service = &mut services[index];
    if let Err(error) = withdrawn {
        // Either way the record is as it was before the withdraw, so the
        // same withdraw from pkg takes the same path again.
        service.withdrawn = false;
        let (line, reply) = error.report(&name, withdraw.version, pid);
        println!("{line}");
        return reply;
    }
    service.start_at_ms = None;
    println!(
        "{name} ends: pkg took package version {} back out",
        withdraw.version
    );
    String::from("ok")
}
