//! Running the scenario's steps against a driver.

use super::{Applied, Driver, STEPS, Step};
use Step::*;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::valid_commit;
use livecore::scenario::{self as live, Expect};
use livecore::store::{Component, Origin};

/// What [`Step::Unchanged`] compares.
fn fingerprint(component: &Component, host: u64) -> String {
    let versions: Vec<String> = component
        .versions()
        .iter()
        .map(|v| format!("{} {:?} {}", v.label, v.origin, v.fetches))
        .collect();
    let ledger = &component.ledger;
    format!(
        "{versions:?} next {} host {host} phase {:?} generation {} counters {:?} history {:?} checkpoint {:?} lineage {:?} outbox {} synced {}",
        component.next_label(),
        ledger.phase,
        ledger.generation,
        ledger.counters,
        ledger.history,
        ledger.checkpoint,
        ledger.lineage,
        component.uploads.outbox.pending(),
        component.uploads.outbox.synced().count(),
    )
}

fn expect_all(what: &str, reply: &str, expects: &[Expect]) -> Result<(), String> {
    match expects.iter().find(|expect| !expect.matches(reply)) {
        Some(expect) => Err(format!("{what:?} answered {reply:?}, expected {expect:?}")),
        None => Ok(()),
    }
}

/// Runs every step; the first that fails ends the run with what went wrong.
pub fn run(driver: &mut dyn Driver) -> Result<(), String> {
    let mut host = driver.host_pid();
    let mut snapshot = None;
    for (index, step) in STEPS.iter().enumerate() {
        run_step(driver, step, &mut host, &mut snapshot)
            .map_err(|why| format!("edit scenario step {}: {why}", index + 1))?;
    }
    Ok(())
}

fn run_step(
    driver: &mut dyn Driver,
    step: &Step,
    host: &mut u64,
    snapshot: &mut Option<String>,
) -> Result<(), String> {
    match *step {
        Live(step) => live::run_step(driver, &step, host)?,
        Edit(line, expects) => expect_all(line, &driver.edit(line)?, expects)?,
        BuildDone(expects) => build_done(driver, expects)?,
        Apply {
            answer,
            view,
            outcome,
        } => match driver.apply(answer)? {
            Applied::Asked {
                view: shown,
                outcome: result,
            } => {
                expect_all(":apply view", &shown, &[view])?;
                expect_all(answer, &result, &[outcome])?;
            }
            Applied::Refused(why) => return Err(format!(":apply refused: {why}")),
        },
        ApplyRefused(expect) => match driver.apply("no")? {
            Applied::Refused(why) => expect_all(":apply", &why, &[expect])?,
            Applied::Asked { view, .. } => {
                return Err(format!(":apply asked when it should refuse: {view}"));
            }
        },
        Revert(args, expect) => expect_all(":revert", &driver.revert(args)?, &[expect])?,
        Uploads(wanted, next) => {
            let component = driver.component();
            let found: Vec<(String, String)> = component
                .versions()
                .iter()
                .filter_map(|v| match &v.origin {
                    Origin::Upload { base, commit, .. } if valid_commit(commit) => {
                        Some((v.label.clone(), base.clone()))
                    }
                    Origin::Upload { .. } => Some((v.label.clone(), String::from("?"))),
                    Origin::Boot => None,
                })
                .collect();
            let wanted: Vec<(String, String)> = wanted
                .iter()
                .map(|(l, b)| (String::from(*l), String::from(*b)))
                .collect();
            if found != wanted || component.next_label() != next {
                return Err(format!(
                    "uploads {found:?} next {}, expected {wanted:?} next {next}",
                    component.next_label()
                ));
            }
        }
        UploadsRefused(wanted) => {
            let found = driver.component().uploads.rejected;
            if found != wanted {
                return Err(format!("{found} uploads rejected, expected {wanted}"));
            }
        }
        Snapshot => *snapshot = Some(fingerprint(&driver.component(), driver.host_pid())),
        Unchanged => {
            let now = fingerprint(&driver.component(), driver.host_pid());
            let before = snapshot.as_deref().ok_or("no snapshot was taken")?;
            if now != before {
                return Err(format!(
                    "the store changed:\n  before {before}\n  after  {now}"
                ));
            }
        }
        Builds(wanted) => {
            let found = driver.builds();
            if found != wanted {
                return Err(format!("{found} builds, expected {wanted}"));
            }
        }
        Synced(label, outcome) => {
            let jobs = driver.settle_sync()?;
            let last = jobs.last().ok_or("git acknowledged no job")?;
            let sha = last.sha.as_deref().unwrap_or("");
            let commit = driver
                .component()
                .version(label)
                .and_then(|v| v.commit().map(String::from));
            let sha_ok = match commit {
                Some(commit) => sha == commit,
                None => valid_commit(sha),
            };
            if last.label != label || last.outcome != outcome || !sha_ok {
                return Err(format!(
                    "the last synced job is {} {:?} at {sha:?}, expected {label} {outcome:?}",
                    last.label, last.outcome
                ));
            }
        }
        Holders => {
            if let Some((publish, component)) = driver.holders()
                && (publish != ["edit"] || component != ["notes"])
            {
                return Err(format!(
                    "PUBLISH is held by {publish:?} and COMPONENT by {component:?}"
                ));
            }
        }
        SpoofEdit(request, expect) => {
            expect_all(request, &driver.spoof_edit(request)?, &[expect])?;
        }
        PutRefused(label) => match driver.put_boot_artifact(label) {
            Err(Error::Invalid) => {}
            other => {
                return Err(format!(
                    "putting boot artifact {label} gave {other:?}, expected Invalid"
                ));
            }
        },
        RestartEditor => driver.restart_editor()?,
        Restored(expect) => {
            let said = driver.restored().ok_or("the kernel restored no session")?;
            expect_all("the restore", &said, &[expect])?;
        }
        RelayRefused(requests) => {
            for request in requests {
                match driver.relay(request) {
                    Err(Error::Invalid) => {}
                    other => {
                        return Err(format!(
                            "relaying {request:?} gave {other:?}, expected Invalid"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// How often BuildDone asks `status`.
const POLL_MS: u64 = 250;
/// Its checks outlast the longest build the kernel waits for.
const BUILD_POLLS: u64 = (crate::service::BUILD_TIMEOUT_MS + 20_000) / POLL_MS;

/// `status` until no build runs; the last build's answer holds every expectation.
fn build_done(driver: &mut dyn Driver, expects: &[Expect]) -> Result<(), String> {
    for _ in 0..BUILD_POLLS {
        let reply = driver.edit("status")?;
        if reply.lines().next().unwrap_or("").contains("building") {
            driver.wait(POLL_MS);
            continue;
        }
        let (_, last) = reply
            .split_once("\nlast build: ")
            .ok_or_else(|| format!("status answered {reply:?}, with no last build"))?;
        return expect_all("the last build", last, expects);
    }
    Err(format!(
        "the build did not finish after {BUILD_POLLS} status checks"
    ))
}
