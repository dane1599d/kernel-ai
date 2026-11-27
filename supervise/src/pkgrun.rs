//! The `run`, `withdraw` and `may-run` requests: how the holder of
//! `/system/pkg` starts and ends `start=pkg` apps on package versions, and
//! the order a withdraw takes so a supervisor that dies between its steps
//! never restarts a withdrawn version.

use crate::{Folder, ServiceSpec, Start, valid_name};
use alloc::format;
use alloc::string::String;

/// `run <name> version=<label>`: the holder of `/system/pkg` asks for `start=pkg`
/// service `name` on package version `label` (`p-` and 12 hex digits), which it
/// added with COMP_ADD. The supervisor takes it from no one else ([`check_run`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run<'a> {
    pub name: &'a str,
    pub version: &'a str,
}

impl<'a> Run<'a> {
    pub fn parse(text: &'a str) -> Option<Self> {
        Self::parse_verb(text, "run")
    }

    pub fn encode(self) -> String {
        format!("run {} version={}", self.name, self.version)
    }

    /// `withdraw <name> version=<label>`: the holder of `/system/pkg` took
    /// package version `label` of `start=pkg` service `name` back out of the
    /// current generation (an undo), so the app ends if it runs that version,
    /// and waits until `pkg` names one again. Gated as a `run` is
    /// ([`check_run`]).
    pub fn parse_withdraw(text: &'a str) -> Option<Self> {
        Self::parse_verb(text, "withdraw")
    }

    pub fn encode_withdraw(self) -> String {
        format!("withdraw {} version={}", self.name, self.version)
    }

    fn parse_verb(text: &'a str, verb: &str) -> Option<Self> {
        let mut words = text.split_whitespace();
        let (Some(said), Some(name), Some(version), None) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            return None;
        };
        let version = version.strip_prefix("version=")?;
        (said == verb && valid_name(name) && kabi::live::is_package_label(version))
            .then_some(Run { name, version })
    }
}

/// The answer to a `run` for a service whose plan line is not `start=pkg`, as a
/// local override's is: `pkg` shows it as shadowed, not as a failure.
pub const NOT_FROM_PKG: &str = "err not a start=pkg service";
/// The answer to a `run` for an app that already serves another version.
pub const RUNS_ANOTHER: &str = "err it runs another version";
/// The answer to a `run` for an app the plan has no line for: the image's
/// repository holds it, but this system does not run it (the v0.1 system has
/// no headless `notes` without a build service). `pkg` shows it, not a failure.
pub const NOT_PLANNED: &str = "err unknown service";

/// `may-run <name>`: whether `pkg` may start service `name` on a package version,
/// asked before it adds one (COMP_ADD), so an app whose plan line runs the
/// image's own version, as every built-in window does in v0.1, is never loaded
/// a second time. Gated as a [`Run`] is ([`check_run_name`]), and it starts
/// nothing: the answer is `ok`, [`NOT_FROM_PKG`] or why not.
pub const MAY_RUN: &str = "may-run";

pub fn parse_may_run(text: &str) -> Option<&str> {
    let mut words = text.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some(MAY_RUN), Some(name), None) if valid_name(name) => Some(name),
        _ => None,
    }
}

pub fn encode_may_run(name: &str) -> String {
    format!("{MAY_RUN} {name}")
}

/// Why [`withdraw_saving_first`] left the app running.
#[derive(Debug, PartialEq, Eq)]
pub enum WithdrawError<E> {
    /// The cleared version could not be saved, so the version was put back and
    /// the app was not ended: ending it would let a supervisor that dies later
    /// restart the withdrawn version from the older saved state.
    NotSaved(E),
    /// The app could not be ended; the version was put back and saved again.
    NotEnded(E),
}

/// The answer to a withdraw whose cleared version could not be saved
/// ([`WithdrawError::NotSaved`]), before `: <error>`.
pub const NOT_SAVED: &str = "err cannot save it";
/// The answer to a withdraw whose app could not be ended
/// ([`WithdrawError::NotEnded`]), before `: <error>`.
pub const NOT_ENDED: &str = "err cannot end it";

/// How many times `pkg` sends one withdraw while the supervisor answers
/// [`NOT_SAVED`] or [`NOT_ENDED`] ([`withdraw_may_retry`]).
pub const WITHDRAW_TRIES: u32 = 3;
/// How long `pkg` waits between those tries.
pub const WITHDRAW_RETRY_MS: u64 = 250;

impl<E: core::fmt::Debug> WithdrawError<E> {
    /// The supervisor's line for service `name`, kept on package `version`
    /// with its `pid` if it runs, and its answer to `pkg`. A failed save
    /// ends nothing, so an app that was not running (crashed and waiting for
    /// its restart, or stopped) is not said to keep running.
    pub fn report(&self, name: &str, version: &str, pid: Option<u64>) -> (String, String) {
        match (self, pid) {
            (Self::NotSaved(error), Some(pid)) => (
                format!(
                    "{name} keeps running package version {version} (pid {pid}): \
                     cannot save it withdrawn: {error:?}"
                ),
                format!("{NOT_SAVED}: {error:?}"),
            ),
            (Self::NotSaved(error), None) => (
                format!(
                    "{name} keeps package version {version}, not running now: \
                     cannot save it withdrawn: {error:?}"
                ),
                format!("{NOT_SAVED}: {error:?}"),
            ),
            (Self::NotEnded(error), pid) => (
                format!(
                    "cannot end {name} (pid {}) on package version {version}: {error:?}",
                    pid.unwrap_or(0)
                ),
                format!("{NOT_ENDED}: {error:?}"),
            ),
        }
    }
}

/// Whether `pkg` may send a withdraw again after `reply`: only after
/// [`NOT_SAVED`] or [`NOT_ENDED`], which leave the record as it was before
/// the withdraw, so the same withdraw takes the same path; never after a
/// refusal or an `ok`.
pub fn withdraw_may_retry(reply: &str) -> bool {
    [NOT_SAVED, NOT_ENDED].iter().any(|said| {
        reply
            .strip_prefix(said)
            .is_some_and(|rest| rest.starts_with(": "))
    })
}

/// Takes a package version back out in the order that survives a supervisor
/// that dies between the steps: the cleared version is saved *before* the app
/// is ended, so a restart never finds the withdrawn version recorded. If that
/// save fails, the version is put back and nothing is ended. If the kill
/// fails, the version is put back and saved again; a failure of that second
/// save is left to `save` to report, as the kill's error is the answer.
/// `clear` returns what it took out; `restore` puts it back.
pub fn withdraw_saving_first<C: ?Sized, V, E>(
    cx: &mut C,
    clear: impl FnOnce(&mut C) -> V,
    restore: impl FnOnce(&mut C, V),
    save: impl Fn(&C) -> Result<(), E>,
    kill: impl FnOnce(&C) -> Result<(), E>,
) -> Result<(), WithdrawError<E>> {
    let taken = clear(cx);
    if let Err(error) = save(cx) {
        restore(cx, taken);
        return Err(WithdrawError::NotSaved(error));
    }
    match kill(cx) {
        Ok(()) => Ok(()),
        Err(error) => {
            restore(cx, taken);
            let _ = save(cx);
            Err(WithdrawError::NotEnded(error))
        }
    }
}

/// [`check_run_name`] for the service `run` names.
pub fn check_run(
    run: Run,
    services: &[(&ServiceSpec, Option<u64>)],
    caller: u64,
) -> Result<usize, &'static str> {
    check_run_name(run.name, services, caller)
}

/// The index among `services` (each spec with its running pid) of service
/// `name`, if `caller` may run a package version of it: the running instance of
/// a service whose plan line grants `/system/pkg` (`dir=pkg`), and only for a
/// `start=pkg` service. The kernel checks the holder's program when it spawns
/// it.
pub fn check_run_name(
    name: &str,
    services: &[(&ServiceSpec, Option<u64>)],
    caller: u64,
) -> Result<usize, &'static str> {
    let holds = services
        .iter()
        .any(|(spec, pid)| *pid == Some(caller) && spec.dirs.contains(&Folder::Pkg));
    if !holds {
        return Err("err only the holder of /system/pkg may run a package");
    }
    let index = services
        .iter()
        .position(|(spec, _)| spec.name == name)
        .ok_or(NOT_PLANNED)?;
    match services[index].0.start {
        Start::Pkg => Ok(index),
        Start::Boot | Start::Demand => Err(NOT_FROM_PKG),
    }
}
