//! Per-app network grants and their asks (P5 task 13, host half; ADR 0001 D7
//! and D8; P5 question 10): what an app's first connection to a new host
//! decides, and the `kai-grants 1` host records that answer it later.
//!
//! - **The ceiling.** An app may only ever reach what its package's `NetDest`
//!   allows (`netdest::Dest`, K's task 1). A host outside it is refused, never
//!   asked about: no answer can widen a handle.
//! - **The ask.** A host inside the ceiling with no grant asks once, naming the
//!   app and the host, on the trusted path's Allow / Deny menu. The menu's
//!   answer, checked against its `ONCE` there, is the only way in
//!   ([`NetGrants::answer`]): the AI can never answer it (ADR 0001 `USER`).
//! - **The grant.** "Allow" keeps a `session` grant for that one host, dropped
//!   at boot; "Always allow on <host>" keeps a `persistent` one (ADR 0001 D7: a
//!   persistent grant needs an explicit "always"). A grant is per host, never a
//!   suffix, so `api.weather.example` grants nothing under it.
//! - **The handle.** The app's `NetDest` is narrowed to its granted hosts
//!   ([`NetGrants::dest_for`]), so the kernel enforces each grant.
//! - **Settings and undo.** [`NetGrants::revoke`] removes a grant and gives it
//!   back for `undo`, which [`NetGrants::restore`] puts back under its own id.
//!
//! Until `powerbox` owns `/system/grants`, the shell keeps these lines in a file
//! of its own, as it keeps its rules; the file is read as hostile and refused
//! whole. A grant id is never reused while the shell runs. A reload numbers on
//! from the highest id the file holds, so an id revoked last and not in the file
//! can come back: `powerbox` keeps the high-water mark when it takes the file.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kaigrants::{From, Grant, Grants, Kind, Life, Time};
use netdest::Dest;
use termcore::menu::{Answer, MAX_TEXT};

/// The action a network ask is about, as the menu's call and the log name it.
pub const ACTION: &str = "net_connect";
/// The rights a host grant holds: a resolve of the name, and a connect to what
/// it resolved.
pub const RIGHTS: [&str; 2] = ["resolve", "connect"];
/// Most host grants kept, every app together.
pub const MAX_GRANTS: usize = 256;

/// What a connection to a host comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// A grant answers it: this one.
    Granted(u32),
    /// No grant yet: the person is asked.
    Ask(Ask),
    /// Never: outside the package's `NetDest`, or not a host name.
    Refused(String),
}

/// One ask, for the trusted path's menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub app: String,
    /// Normalised: lower case, no trailing dot.
    pub host: String,
    /// The menu's question, which names both.
    pub question: String,
    /// The call the menu's `ONCE` names: `net_connect("weather", "api.weather.example")`.
    pub call: String,
}

/// The shell's host grants.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetGrants {
    grants: Vec<Grant>,
    next: u32,
}

/// The canonical call an ask names.
fn call(app: &str, host: &str) -> String {
    format!("{ACTION}(\"{app}\", \"{host}\")")
}

/// A package name as `kai-grants 1` takes one.
fn app_ok(app: &str) -> bool {
    !app.is_empty()
        && app
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl NetGrants {
    pub fn new() -> Self {
        Self::default()
    }

    /// The host grants a `kai-grants 1` file holds; its other lines are not
    /// this store's, so a file with any is refused.
    pub fn load(text: &str) -> Result<Self, String> {
        let read = kaigrants::parse(text).map_err(|e| format!("line {}: {}", e.line, e.why))?;
        if !read.rules.is_empty() || read.grants.iter().any(|g| g.kind != Kind::Host) {
            return Err(String::from(
                "the network grants file holds a line not its own",
            ));
        }
        if read.grants.len() > MAX_GRANTS {
            return Err(String::from(
                "the network grants file holds too many grants",
            ));
        }
        for grant in &read.grants {
            if netdest::normalize(&grant.object).as_deref() != Some(grant.object.as_str()) {
                return Err(format!("grant {}: not a host name", grant.id));
            }
        }
        let next = read.grants.iter().map(|g| g.id).max().unwrap_or(0);
        Ok(Self {
            grants: read.grants,
            next,
        })
    }

    /// The grants as their file keeps them.
    pub fn text(&self) -> Result<String, String> {
        Grants {
            grants: self.grants.clone(),
            rules: Vec::new(),
        }
        .text()
    }

    /// What a boot keeps: `session` grants go.
    pub fn at_boot(&mut self) {
        self.grants.retain(|g| g.life == Life::Persistent);
    }

    /// App `app`'s grants, for Settings.
    pub fn of(&self, app: &str) -> Vec<&Grant> {
        self.grants.iter().filter(|g| g.app == app).collect()
    }

    /// What app `app`'s connection to `host` comes to, under `ceiling`, its
    /// package's `NetDest`.
    pub fn decide(&self, app: &str, host: &str, ceiling: &Dest) -> Decision {
        if !app_ok(app) {
            return Decision::Refused(String::from("not an app's name"));
        }
        let Some(host) = netdest::normalize(host) else {
            return Decision::Refused(String::from("not a host name"));
        };
        if !ceiling.allows_name(&host) {
            return Decision::Refused(format!("{app}'s package does not name {host}"));
        }
        if let Some(grant) = self.find(app, &host) {
            return Decision::Granted(grant.id);
        }
        let question = format!("Let {app} connect to {host}?");
        let call = call(app, &host);
        // The menu shows the question and the call whole, or not at all.
        if question.len() > MAX_TEXT || call.len() > MAX_TEXT {
            return Decision::Refused(format!(
                "{host} is too long a name to ask about on the card"
            ));
        }
        Decision::Ask(Ask {
            app: app.to_string(),
            host,
            question,
            call,
        })
    }

    fn find(&self, app: &str, host: &str) -> Option<&Grant> {
        self.grants
            .iter()
            .find(|g| g.app == app && g.object == host)
    }

    /// What the menu's answer to `ask` keeps: the grant's id, or `None` for a
    /// Deny. Only the trusted path's answer, its `ONCE` checked, may come here.
    /// "Always allow" is offered for the host alone, never for every host.
    pub fn answer(
        &mut self,
        ask: &Ask,
        answer: Answer,
        request: u64,
        now_ns: u64,
    ) -> Result<Option<u32>, &'static str> {
        let life = match answer {
            Answer::Deny => return Ok(None),
            Answer::Allow => Life::Session,
            Answer::AlwaysTarget => Life::Persistent,
            Answer::AlwaysKind => return Err("a grant for every host was not offered"),
        };
        if let Some(grant) = self
            .grants
            .iter_mut()
            .find(|g| g.app == ask.app && g.object == ask.host)
        {
            // An "always" makes a session grant persistent; nothing else changes it.
            if life == Life::Persistent {
                grant.life = Life::Persistent;
            }
            return Ok(Some(grant.id));
        }
        if self.grants.len() >= MAX_GRANTS {
            return Err("there are too many network grants; remove one in Settings");
        }
        let id = self.next.checked_add(1).ok_or("grant ids are used up")?;
        self.next = id;
        self.grants.push(Grant {
            id,
            app: ask.app.clone(),
            kind: Kind::Host,
            object: ask.host.clone(),
            rights: RIGHTS.iter().map(|r| r.to_string()).collect(),
            from: From::Request(request),
            life,
            signer: None,
            made: Time::from_ns(now_ns),
        });
        Ok(Some(id))
    }

    /// App `app`'s `NetDest`: `ceiling` narrowed to the hosts it was granted,
    /// or `None` when it has none (the app reaches no host by name).
    pub fn dest_for(&self, app: &str, ceiling: &Dest) -> Option<Dest> {
        let hosts: Vec<String> = self.of(app).iter().map(|g| g.object.clone()).collect();
        if hosts.is_empty() {
            return None;
        }
        let narrowed = Dest {
            hosts,
            ..ceiling.clone()
        };
        // Each grant was within the ceiling when it was made; one that is not
        // now (the package changed) gives nothing.
        narrowed.within(ceiling).then_some(narrowed)
    }

    /// Removes grant `id` (Settings): the grant, for `undo` to put back.
    pub fn revoke(&mut self, id: u32) -> Option<Grant> {
        let at = self.grants.iter().position(|g| g.id == id)?;
        Some(self.grants.remove(at))
    }

    /// Puts back a revoked grant under its own id (`undo` of a revoke). Refused
    /// when the id or the app's host is held again since.
    pub fn restore(&mut self, grant: Grant) -> Result<(), &'static str> {
        if self.grants.iter().any(|g| g.id == grant.id) {
            return Err("that grant's id is in use");
        }
        if self.find(&grant.app, &grant.object).is_some() {
            return Err("the app was granted that host again since");
        }
        if self.grants.len() >= MAX_GRANTS {
            return Err("there are too many network grants; remove one in Settings");
        }
        self.next = self.next.max(grant.id);
        self.grants.push(grant);
        Ok(())
    }
}
