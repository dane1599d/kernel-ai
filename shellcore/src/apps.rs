//! Installed apps' action rows (P4, the agent-to-app route): what an app's
//! `kai-actions 1` answer declares, checked and turned into ADR 0002 rows with
//! owner `App(<app>)`, so the agent core decides, runs, logs and undoes them as
//! it does the OS's.
//!
//! The rows come from the artifact comphost loaded (`livecore::rows`), never
//! from the app's code. Each name must start with `<app>_`, where `<app>` is
//! the kernel's name for the process, not anything the app says. A row's tier
//! is the OS's, from its facts: a fact the app left out takes ADR 0002's
//! pessimistic default, so the app can only make a tier stricter.
//!
//! The rows are leaked into `'static` to fit ADR 0002's static row type, at
//! most [`MAX_ROWS`] a version and [`MAX_LOADS`] versions in all until reboot;
//! past that a load is refused (owned rows replace this: ADR 0002 and 0003).

use agentabi::{Action, Arg, ArgType, Facts, Inverse, Owner, RetType, Returns, Role, Tier};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use appabi::Ty;
use kabi::live::NOTICE_TICKET;
use livecore::actions::{self as declared, ActionRow, ArgKind};

mod rows;
pub use rows::*;

/// Most rows one app version may declare.
pub const MAX_ROWS: usize = 64;
/// Most app versions whose rows load until reboot.
pub const MAX_LOADS: usize = 32;
/// The longest text a row's text argument takes when the app sets no `max=`.
const TEXT_MAX: u16 = 2000;
/// The most items a list argument takes.
const LIST_MAX: u8 = 32;

/// The shell's apps: their rows, and the effects their calls queued, which
/// the agent core runs once the call's run succeeded.
#[derive(Default)]
pub struct Apps {
    pub rows: AppRows,
    pub effects: Vec<(String, String)>,
    /// What those effects came to, by app, for the host to hand back to the app
    /// as `effect-done`.
    pub results: Vec<(String, Vec<crate::effects::EffectDone>)>,
}

/// Batches of results kept for the host to hand back, in all and for one app:
/// past either the oldest go, and the shell says so.
const MAX_RESULT_BATCHES: usize = 16;
const MAX_RESULT_BATCHES_PER_APP: usize = 8;

impl Apps {
    /// What the runs queued since the last take, as (app, effect lines).
    pub fn take_effects(&mut self) -> Vec<(String, String)> {
        core::mem::take(&mut self.effects)
    }

    /// Keeps what `app`'s effects came to, until the host takes it. Past
    /// [`MAX_RESULT_BATCHES_PER_APP`] of the app's own, or [`MAX_RESULT_BATCHES`]
    /// in all, the oldest batch goes, and its app's name comes back for the
    /// caller to say so: nothing is dropped silently.
    pub fn keep_results(
        &mut self,
        app: &str,
        done: Vec<crate::effects::EffectDone>,
    ) -> Option<String> {
        if done.is_empty() {
            return None;
        }
        let own = self.results.iter().filter(|(name, _)| name == app).count();
        let oldest = if own == MAX_RESULT_BATCHES_PER_APP {
            self.results.iter().position(|(name, _)| name == app)
        } else if self.results.len() == MAX_RESULT_BATCHES {
            Some(0)
        } else {
            None
        };
        let dropped = oldest.map(|at| self.results.remove(at).0);
        self.results.push((app.to_string(), done));
        dropped
    }

    /// Keeps a notice (ticket [`NOTICE_TICKET`], outcome `text`) for `app`, unless
    /// one that says the same waits already: it says only that something changed,
    /// so a burst of changes is one notice. Past the caps, as [`Apps::keep_results`].
    pub fn keep_notice(&mut self, app: &str, text: &str) -> Option<String> {
        let told = |done: &crate::effects::EffectDone| {
            done.ticket == NOTICE_TICKET && done.outcome.as_deref() == Ok(text)
        };
        if self
            .results
            .iter()
            .any(|(name, batch)| name == app && batch.iter().any(told))
        {
            return None;
        }
        let notice = crate::effects::EffectDone {
            ticket: NOTICE_TICKET,
            outcome: Ok(text.to_string()),
        };
        self.keep_results(app, alloc::vec![notice])
    }

    /// What the effects run since the last take came to, as (app, results).
    pub fn take_results(&mut self) -> Vec<(String, Vec<crate::effects::EffectDone>)> {
        core::mem::take(&mut self.results)
    }
}

/// Loads the rows of the app whose `<app>_` names `name`, if a service of
/// that name answers `kai-actions 1`: the first call to an app's row loads its
/// rows, so no app starts only to be listed. The longest name wins, so
/// `notes_window_add` is `notes-window`'s before `notes`'s.
pub(crate) fn load_for(host: &dyn crate::Host, name: &str) {
    let Some(apps) = host.apps() else {
        return;
    };
    let Ok(services) = host.services() else {
        return;
    };
    let starts = |pkg: &str| name.starts_with(&format!("{}_", pkg.replace('-', "_")));
    // A service named for the row's start, the longest; else a window that
    // fronts that package (`notes_add` is `notes-window`'s with no `notes`).
    let named = services
        .iter()
        .map(|s| s.name.as_str())
        .filter(|app| starts(app))
        .max_by_key(|app| app.len());
    let window = || {
        services
            .iter()
            .map(|s| s.name.as_str())
            .filter(|app| app.strip_suffix("-window").is_some_and(starts))
            .max_by_key(|app| app.len())
    };
    let Some(app) = named.or_else(window) else {
        return;
    };
    if apps.borrow().rows.has(app) || apps.borrow().rows.failed(app) {
        return;
    }
    let others: Vec<String> = services
        .iter()
        .map(|s| s.name.clone())
        .filter(|name| name != app)
        .collect();
    let answer = match host.app_call(app, "kai-actions 1") {
        Ok(answer) => answer,
        Err(why) => return host.audit(&format!("{app}'s rows were not asked for: {why}")),
    };
    let loaded = apps.borrow_mut().rows.load_among(app, &answer, &others);
    match loaded {
        Ok(count) => host.audit(&format!("{app}'s {count} actions loaded")),
        Err(why) => host.audit(&why),
    }
}

/// The `apps` text of an `act` request (P4 task 15): the `kai-actions 1`
/// answers of the running apps, so the model sees their rows
/// (`llmproto::pack::apps`). Each running instance is asked once; a service
/// that is not an app is not asked again until reboot, and one that is not
/// running is neither asked (that would start it) nor shown.
pub(crate) fn wire(host: &dyn crate::Host) -> String {
    let (Some(apps), Ok(services)) = (host.apps(), host.services()) else {
        return String::new();
    };
    let running: Vec<&crate::targets::Service> = services
        .iter()
        .filter(|s| s.pid != 0 && !s.stopped)
        .collect();
    for service in &running {
        let app = service.name.as_str();
        let known = {
            let rows = &apps.borrow().rows;
            rows.failed(app) || rows.asked(app) == Some(service.pid)
        };
        if known {
            continue;
        }
        apps.borrow_mut()
            .rows
            .asked
            .push((app.to_string(), service.pid));
        let answer = match host.app_call(app, livecore::rows::REQUEST) {
            Ok(answer) => answer,
            Err(why) if why == NOT_AN_APP => {
                apps.borrow_mut().rows.failed.push(app.to_string());
                continue;
            }
            Err(why) => {
                host.audit(&format!("{app}'s rows were not asked for: {why}"));
                continue;
            }
        };
        let others: Vec<String> = services
            .iter()
            .map(|s| s.name.clone())
            .filter(|name| name != app)
            .collect();
        let loaded = apps.borrow_mut().rows.load_among(app, &answer, &others);
        match loaded {
            Ok(count) => host.audit(&format!("{app}'s {count} actions loaded")),
            Err(why) => host.audit(&why),
        }
    }
    installed(host, apps, &services);
    let apps = apps.borrow();
    // Running apps first, then installed ones whose rows came from `pkg`.
    let resting = services.iter().filter(|s| s.pid == 0 || s.stopped);
    let answers: Vec<(&str, &str)> = running
        .iter()
        .copied()
        .chain(resting)
        .filter_map(|s| Some((s.name.as_str(), apps.rows.answer(&s.name)?)))
        .collect();
    let (text, left_out) = llmproto::pack::apps::wire(&answers, llmproto::act::APPS_MAX_BYTES);
    if !left_out.is_empty() {
        host.audit(&format!(
            "past one act request, left out of the prompt: {}",
            left_out.join(" ")
        ));
    }
    text
}

/// Whether `service` is an app, which only the supervisor knows (an app is
/// one it runs as a component). Only its "no such app" says not: the shell,
/// llm. One whose rows are loaded is an app. "No such app" is remembered for
/// that running instance (its pid), so a service restarted, as an app
/// installed under its name would be, is asked again.
///
/// A service that is not running is not asked, which would start it: it is
/// an app unless it said "no such app" since boot (#914's review: llm down
/// is still the model's). One whose answer is any other error (slow, busy)
/// counts as an app, as before this check, and is not asked again for
/// [`SLOW_FOR_NS`], so a hung app does not hold up each `open`.
pub(crate) fn is_app(host: &dyn crate::Host, service: &crate::targets::Service) -> bool {
    let app = service.name.as_str();
    let now = host.now_ns();
    if let Some(apps) = host.apps() {
        let apps = apps.borrow();
        if apps.rows.has(app) {
            return true;
        }
        if service.pid == 0 || service.stopped {
            return !apps.rows.said_not_app(app);
        }
        if apps.rows.not_app(app, service.pid) {
            return false;
        }
        if apps.rows.slow(app, service.pid, now) {
            return true;
        }
    } else if service.pid == 0 || service.stopped {
        return true;
    }
    let answer = host.app_call(app, livecore::rows::REQUEST);
    let Some(apps) = host.apps() else {
        return !matches!(answer, Err(ref why) if why == NOT_AN_APP);
    };
    let rows = &mut apps.borrow_mut().rows;
    match answer {
        Err(why) if why == NOT_AN_APP => {
            rows.not_apps.push((app.to_string(), service.pid));
            false
        }
        Err(_) => {
            rows.slow_since(app, service.pid, now);
            true
        }
        Ok(_) => true,
    }
}

/// How long an app that answered [`is_app`] with an error other than "no
/// such app" is not asked again.
pub const SLOW_FOR_NS: u64 = 30_000_000_000;
/// The most slow answers kept; the oldest goes first.
const MAX_SLOW: usize = 16;

/// The rows of installed apps that are not running, read by `pkg` from their
/// stored artifacts (founder test 2, item 3): "make a note called ideas" on a
/// fresh boot must see `notes_add`, and a call to it starts the app
/// ([`load_for`]). Each is asked once until reboot; a service `pkg` did not
/// install is simply not shown.
fn installed(
    host: &dyn crate::Host,
    apps: &core::cell::RefCell<Apps>,
    services: &[crate::targets::Service],
) {
    for service in services.iter().filter(|s| s.pid == 0 || s.stopped) {
        let app = service.name.as_str();
        let known = {
            let rows = &apps.borrow().rows;
            rows.has(app) || rows.failed(app) || rows.asked(app).is_some()
        };
        if known {
            continue;
        }
        apps.borrow_mut().rows.asked.push((app.to_string(), 0));
        // A window's service, `<x>-window`, runs package `<x>` (kaipkg).
        let package = app.strip_suffix("-window").unwrap_or(app);
        let answer = match host.installed_rows(package) {
            Ok(answer) => answer,
            // A service `pkg` did not install: asked, and that is the answer.
            Err(why) if why.ends_with(NOT_INSTALLED) => continue,
            // `pkg` gave no answer: asked again on a later act turn, up to
            // [`INSTALLED_TRIES`] in all.
            Err(why) if why.starts_with(crate::ccbuild::NO_ANSWER) => {
                host.audit(&format!("{app}'s package rows were not read: {why}"));
                let mut apps = apps.borrow_mut();
                apps.rows.unanswered.push(app.to_string());
                let tries = apps.rows.unanswered.iter().filter(|a| *a == app).count();
                if tries < INSTALLED_TRIES {
                    apps.rows.asked.retain(|(name, _)| name != app);
                }
                continue;
            }
            // Any answer of `pkg`'s, or no route to it, is final until reboot:
            // it stays asked, so it is audited once. (Not `failed`, which
            // would keep the app's own rows out once it runs.)
            Err(why) => {
                host.audit(&format!("{app}'s package rows were not read: {why}"));
                continue;
            }
        };
        let others: Vec<String> = services
            .iter()
            .map(|s| s.name.clone())
            .filter(|name| name != app)
            .collect();
        let loaded = apps.borrow_mut().rows.load_among(app, &answer, &others);
        match loaded {
            Ok(count) => host.audit(&format!("{app}'s {count} actions loaded from its package")),
            Err(why) => host.audit(&why),
        }
    }
}

/// The supervisor's answer for a service that is not an app
/// (`supervise::appcall::check`), less its `err `.
const NOT_AN_APP: &str = "no such app";

/// The end of `pkg`'s answer for a package it has not installed (`pkg`'s
/// `actions`).
const NOT_INSTALLED: &str = " is not installed";

/// How many times an installed app's rows are asked of a `pkg` that gives no
/// answer, each costing up to its call timeout, before the shell stops asking.
const INSTALLED_TRIES: usize = 3;

/// Loads app `app`'s rows, by its name, if they are not loaded yet: for its
/// effects, which may come before any call to its rows.
pub(crate) fn load_app(host: &dyn crate::Host, app: &str) {
    let Some(apps) = host.apps() else {
        return;
    };
    if apps.borrow().rows.has(app) || apps.borrow().rows.failed(app) {
        return;
    }
    let others: Vec<String> = host
        .services()
        .map(|all| {
            all.into_iter()
                .map(|s| s.name)
                .filter(|n| n != app)
                .collect()
        })
        .unwrap_or_default();
    let answer = match host.app_call(app, "kai-actions 1") {
        Ok(answer) => answer,
        Err(why) => return host.audit(&format!("{app}'s rows were not asked for: {why}")),
    };
    let loaded = apps.borrow_mut().rows.load_among(app, &answer, &others);
    match loaded {
        Ok(count) => host.audit(&format!("{app}'s {count} actions loaded")),
        Err(why) => host.audit(&why),
    }
}

/// `kai-undo 1` of app `app`'s call kept as `id`, with `force` if `anyway`:
/// the undo's own id, which undoes it again, or why not.
pub(crate) fn undo(
    host: &dyn crate::Host,
    app: &str,
    id: u64,
    anyway: bool,
) -> Result<u64, crate::undo::Refused> {
    use crate::undo::Refused;
    let force = if anyway { "force\n" } else { "" };
    let request = format!("kai-undo 1\nid {id}\n{force}");
    let text = host.app_call(app, &request).map_err(Refused::Failed)?;
    let mut lines = text.lines();
    if lines.next() != Some(livecore::call::REPLY_HEADER) {
        return Err(Refused::Failed(format!("{app} did not answer the undo")));
    }
    let mut redo = None;
    for line in lines {
        match line.split_once(' ') {
            Some(("undone", _)) => {}
            Some(("undo", next)) => redo = next.parse().ok(),
            Some(("changed", _)) => {
                return Err(Refused::Moved(format!("{app} changed since")));
            }
            _ => return Err(Refused::Failed(format!("{app}: {line}"))),
        }
    }
    redo.ok_or_else(|| Refused::Failed(format!("{app} kept no undo of its undo")))
}

/// An app as people name it: its package, not its `-window` service, with a
/// capital: `Notes` for `notes-window`.
pub(crate) fn title(service: &str) -> String {
    let name = service.strip_suffix("-window").unwrap_or(service);
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_ascii_uppercase())
        .into_iter()
        .chain(chars)
        .collect()
}

#[cfg(test)]
mod tests;
