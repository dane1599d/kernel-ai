//! The Settings app's `kai:settings@0.1.0` requests, answered by the shell
//! (P4 task 7, A-core's half). comphost forwards a request with the kernel's
//! name for the app that sent it, and with `user` true only when a real key or
//! click started the call (never the agent's `ui_act`). Only the Settings app
//! may ask. Reads answer at once; a change needs `user`, runs as its typed
//! command does, and is logged as the user's click (`consent=user-click:settings`),
//! so `undo` puts it back. The grammar is exact, and every argument is
//! untrusted: a verb not listed, a name out of shape or a number with a sign is
//! refused.
//!
//! Keep-running (the founder's per-app toggle, 2026-10-28): the shell keeps the
//! apps that must not nap in `/system/state/keep`, replays them to the
//! supervisor at boot as `keep <app> on`, and a change sends `keep <app> on|off`.

use crate::actlog::{Change, Entry, Form, State};
use crate::tiers::Tier;
use crate::undo::{Inverse, Name};
use crate::{Reply, Shell};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The app that holds the settings grant.
pub const SETTINGS_APP: &str = "settings";
/// The longest request (`kai:settings`' own bound).
pub const MAX_REQUEST: usize = 512;
/// The longest app or profile name.
const MAX_NAME: usize = 24;
/// The services that are the system's, not apps that nap.
const SYSTEM: &[&str] = &[
    "shell",
    "supervisor",
    "llm",
    "fs",
    "desktop",
    "pkg",
    "updated",
    "edit",
    "build",
    "scriptrun",
    "powerbox",
];
/// The consent a Settings change is logged with.
const CLICK: &str = "user-click:settings";
/// The first line of `/system/state/keep`.
const KEEP_HEADER: &str = "kai-keep 1";

impl Shell {
    /// Answers a `kai:settings` request from app `app`: the answer's text, or
    /// why not.
    pub fn settings(&mut self, app: &str, request: &str, user: bool) -> Result<String, String> {
        if app != SETTINGS_APP {
            return Err(String::from("only the Settings app asks for settings"));
        }
        if request.len() > MAX_REQUEST || request.chars().any(char::is_control) {
            return Err(String::from("not a settings request"));
        }
        let (verb, rest) = request.split_once(' ').unwrap_or((request, ""));
        let change = matches!(
            verb,
            "appearance-set"
                | "profile-set"
                | "rules-remove"
                | "grants-revoke"
                | "grants-restore"
                | "keep-running"
                | "restart-window"
        );
        if change && !user {
            return Err(String::from("a change needs a key or a click"));
        }
        let logged = format!("settings: {request}");
        match (verb, rest) {
            ("rules", "") => Ok(self.rules_lines()),
            // What each app's effects may call: its declared rows, each allowed
            // or withheld (P4 task 7). Built-in imports and its own storage are
            // not rows, so they are not listed or revocable in v0.1.
            ("grants", "") => Ok(self.grant_lines(None)),
            ("grants", name) if valid_name(name) => Ok(self.grant_lines(Some(name))),
            ("grants-revoke" | "grants-restore", rest) => {
                let (name, row) = rest
                    .split_once(' ')
                    .ok_or_else(|| format!("{verb} takes an app and a row"))?;
                self.grant_by_click(&logged, name, row, verb == "grants-revoke")
            }
            ("about", "") => Ok(about()),
            ("profiles", "") => Ok(self.profile_lines()),
            ("apps", "") => self.app_names().map(|names| names.join("\n")),
            ("file", _) => Err(String::from("comphost reads the settings files itself")),
            ("profile-set", name) if valid_name(name) => self
                .profile_set_by_click(&logged, name)
                .map(|was| format!("profile {name} (was {was})")),
            ("rules-remove", which) if valid_rule(which) => {
                let reply = match which.starts_with('S') {
                    true => self.remove_instruction(&logged, which),
                    false => self.remove_rule(which),
                };
                answer(reply)
            }
            ("keep-running", rest) => {
                let (word, name) = rest.split_once(' ').unwrap_or((rest, ""));
                let on = match word {
                    "on" => true,
                    "off" => false,
                    _ => return Err(String::from("keep-running is on or off, then an app")),
                };
                if !valid_name(name) {
                    return Err(String::from("not an app name"));
                }
                self.keep_running(&logged, name, on)
            }
            ("appearance-set" | "restart-window", _) => {
                Err(format!("{verb} is not answered here yet"))
            }
            _ => Err(String::from("not a settings request")),
        }
    }

    /// `rules` for the app: one line a rule, `#<n> ...` or `S<n> ...`; none is
    /// an empty answer.
    fn rules_lines(&self) -> String {
        match self.rules.is_empty() && self.standing.rules().is_empty() {
            true => String::new(),
            false => self.list_rules(),
        }
    }

    /// The apps that can nap: every service the supervisor runs that is not
    /// the system's.
    fn app_names(&self) -> Result<Vec<String>, String> {
        let services = self.host.services()?;
        Ok(services
            .into_iter()
            .map(|s| s.name)
            .filter(|name| !SYSTEM.contains(&name.as_str()))
            .collect())
    }

    fn profile_set_by_click(&mut self, request: &str, name: &str) -> Result<String, String> {
        let id = self.next_id;
        self.next_id += 1;
        let logged = crate::profiles::Logged {
            form: Form::Call,
            consent: String::from(CLICK),
            request,
        };
        self.switch_profile(id, name, logged).map(|(was, _)| was)
    }

    /// Keeps app `name` running (`on`) or lets it nap: the supervisor is told,
    /// the list is saved, and the change is logged with its undo.
    fn keep_running(&mut self, request: &str, name: &str, on: bool) -> Result<String, String> {
        if !self.app_names()?.iter().any(|app| app == name) {
            return Err(format!("{name} is no app that can nap"));
        }
        let was = self.keep.iter().any(|app| app == name);
        if was == on {
            return Ok(format!("{name} keep-running is {} already", word(on)));
        }
        self.set_keep(name, on)?;
        let app = Name::new(name).ok_or_else(|| String::from("not an app name"))?;
        let id = self.next_id;
        self.next_id += 1;
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: Some(Tier::Run),
            pack: None,
            consent: String::from(CLICK),
            state: State::Done,
            request: String::from(request),
            script: format!("app_keep_running(\"{name}\", {on})"),
            outcome: format!("{name} keep-running {}", word(on)),
            changes: alloc::vec![Change {
                action: String::from("app_keep_running"),
                owner: String::from("supervisor"),
                res: format!("keep-running {name}"),
                before: String::from(word(was)),
                after: String::from(word(on)),
                inverse: Some(Inverse::Keep { app, on: was }),
            }],
        });
        self.save_log();
        Ok(format!("{name} keep-running {}", word(on)))
    }

    /// `grants`: one line a declared effect row, `<app>\t<row>\tallowed` or
    /// `withheld`, for `only` or every app whose rows are loaded.
    fn grant_lines(&self, only: Option<&str>) -> String {
        if let Some(app) = only {
            crate::apps::load_app(self.host.as_ref(), app);
        }
        let host = self.host.as_ref();
        let Some(apps) = host.apps() else {
            return String::new();
        };
        let names: Vec<String> = match only {
            Some(app) => alloc::vec![app.to_string()],
            None => apps.borrow().rows.apps().map(str::to_string).collect(),
        };
        let mut lines = Vec::new();
        for app in &names {
            let allowed = crate::grants::allowed(host, app);
            for row in apps.borrow().rows.effects_of(app) {
                let state = match allowed.contains(row) {
                    true => "allowed",
                    false => "withheld",
                };
                lines.push(format!("{app}\t{row}\t{state}"));
            }
        }
        lines.join("\n")
    }

    /// `grants-revoke` or `grants-restore` from a click: `row` withheld from
    /// `name`'s effects or given back, logged as the user's click with its
    /// undo. Revoking takes only a row the app declares.
    fn grant_by_click(
        &mut self,
        request: &str,
        name: &str,
        row: &str,
        withhold: bool,
    ) -> Result<String, String> {
        // The names first: nothing changes, and nothing is logged, for a pair
        // the log and its undo could not hold.
        let named = valid_name(name) && crate::grants::valid_row(row);
        let (true, Some(app), Some(row_name)) = (named, Name::new(name), Name::new(row)) else {
            return Err(String::from("not an app and a row"));
        };
        let host = self.host.as_ref();
        let changed = match withhold {
            true => crate::grants::revoke(host, name, row)?,
            false => crate::grants::set(host, name, row, false)?,
        };
        let (was, now) = match withhold {
            true => ("allowed", "withheld"),
            false => ("withheld", "allowed"),
        };
        if !changed {
            return Ok(format!("{name} {row} is {now} already"));
        }
        let action = if withhold {
            "grant_revoke"
        } else {
            "grant_restore"
        };
        let id = self.next_id;
        self.next_id += 1;
        self.log.push(Entry {
            id,
            t_ns: self.host.now_ns(),
            form: Form::Call,
            of: None,
            tier: Some(Tier::Run),
            pack: None,
            consent: String::from(CLICK),
            state: State::Done,
            request: String::from(request),
            script: format!("{action}(\"{name}\", \"{row}\")"),
            outcome: format!("{name} {row} {now}"),
            changes: alloc::vec![Change {
                action: String::from(action),
                owner: String::from("shell"),
                res: format!("{name}'s effects calling {row}"),
                before: String::from(was),
                after: String::from(now),
                inverse: Some(Inverse::Grant {
                    app,
                    row: row_name,
                    withheld: !withhold,
                }),
            }],
        });
        self.save_log();
        Ok(format!("{name} {row} {now}"))
    }

    /// Tells the supervisor and saves the list: `name` kept running or not.
    pub(crate) fn set_keep(&mut self, name: &str, on: bool) -> Result<(), String> {
        self.host.keep_running(name, on)?;
        self.keep.retain(|app| app != name);
        if on {
            self.keep.push(name.to_string());
        }
        let text = keep_text(&self.keep);
        if let Err(why) = self.host.keep_write(&text) {
            self.host
                .audit(&format!("the keep-running list was not saved: {why}"));
        }
        Ok(())
    }

    /// `undo` of a keep-running change: puts `on` back; its text.
    pub(crate) fn revert_keep(&mut self, name: &str, on: bool) -> Result<String, String> {
        self.set_keep(name, on)?;
        Ok(format!("{name} keep-running {} again", word(on)))
    }

    /// The keep-running list `/system/state/keep` holds, each told to the
    /// supervisor again; a list out of shape is set aside whole.
    pub(crate) fn with_saved_keep(mut self) -> Self {
        let Some(text) = self.host.keep_read() else {
            return self;
        };
        let Some(apps) = parse_keep(&text) else {
            self.host
                .audit("the keep-running list is out of shape; no app is kept");
            return self;
        };
        for app in apps {
            match self.host.keep_running(&app, true) {
                Ok(()) => self.keep.push(app),
                Err(why) => self
                    .host
                    .audit(&format!("{app} is not kept running: {why}")),
            }
        }
        self
    }
}

fn answer(reply: Reply) -> Result<String, String> {
    match reply.ok {
        true => Ok(reply.text),
        false => Err(reply.text),
    }
}

fn word(on: bool) -> &'static str {
    match on {
        true => "on",
        false => "off",
    }
}

/// A name as an app or profile has: 1 to 24 of `a-z`, `0-9` and `-`.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `<n>` or `S<n>`: decimal digits, no sign, a few at most.
fn valid_rule(which: &str) -> bool {
    let digits = which.strip_prefix('S').unwrap_or(which);
    !digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|b| b.is_ascii_digit())
}

/// `key: value` lines about this build.
fn about() -> String {
    format!(
        "version: {}\ncommit: {}\nlicence: MPL-2.0\nsource: https://github.com/dane1599d/kernel-ai\nnotices: NOTICES.txt",
        env!("CARGO_PKG_VERSION"),
        option_env!("KAI_COMMIT").unwrap_or("unknown")
    )
}

fn keep_text(apps: &[String]) -> String {
    let mut text = String::from(KEEP_HEADER);
    for app in apps {
        text.push('\n');
        text.push_str(app);
    }
    text.push('\n');
    text
}

fn parse_keep(text: &str) -> Option<Vec<String>> {
    let mut lines = text.lines();
    if lines.next() != Some(KEEP_HEADER) {
        return None;
    }
    let apps: Vec<String> = lines.filter(|l| !l.is_empty()).map(String::from).collect();
    match apps.iter().all(|app| valid_name(app)) && apps.len() <= 64 {
        true => Some(apps),
        false => None,
    }
}
