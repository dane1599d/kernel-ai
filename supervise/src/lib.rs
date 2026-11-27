//! The ring-3 supervisor's decisions, kept free of system calls so they can be
//! tested on the host: reading the service plan, the restart policy (exponential
//! backoff with a restart-intensity cap, as in Erlang/OTP), and the state saved in
//! the kernel-held region so a restarted supervisor carries on where it left off.

#![no_std]

extern crate alloc;

pub mod keep;
pub mod profile;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::Limits;
#[cfg(test)]
use kabi::cap;

pub mod appcall;
pub mod budget;
mod pkgrun;
mod plan;
mod state;

pub use pkgrun::*;
pub use plan::*;
pub use state::*;

/// Defaults for a service that does not set its own limits.
pub const DEFAULT_PAGES: u64 = 1024;
pub const DEFAULT_BURST_MS: u64 = 2000;

/// One service from the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    pub name: String,
    /// The program to run; the service name unless the plan says `program=`.
    pub program: String,
    /// Arguments passed through to the program.
    pub args: String,
    pub limits: Limits,
    /// The services it may call, from `uses=`: one route each, in this order
    /// (ADR 0001 D3). Each is listed earlier in the plan, so routes form no cycle.
    pub uses: Vec<String>,
    /// The folders it gets a startup `Dir` for, from `dir=`, in this order.
    pub dirs: Vec<Folder>,
    /// The boot files it may read with BOOT_READ, from `bootfiles=`.
    pub boot: Option<BootFiles>,
    /// Its own network destination, from `netdest=` (P5 task 1).
    pub netdest: Option<netdest::Dest>,
    /// From `netdest=llm`: a handle to the LLM server's destination
    /// (`kabi::handle::netdest::LLM`), the one HTTP reaches. Only a line that
    /// asks gets it; `caps=net` alone does not.
    pub llm_dest: bool,
    /// From `agent=<name>`: the agent this line runs (ADR 0013 section 1), whose
    /// badge (`kabi::handle::agent`) the kernel puts on its route to `llm`.
    pub agent: Option<u64>,
    /// From `net_mib=`: the MiB a day its `netdest=` may move, both ways
    /// (P5 task 14, an agent's manifest's `bytes/day`); the kernel counts them
    /// on the handle's record. None: not counted.
    pub net_mib: Option<u32>,
    /// When the supervisor starts it, from `start=`.
    pub start: Start,
    /// Restart policy; the plan may change `max_backoff_ms` and `max_restarts`.
    pub policy: Policy,
    /// From the word `unsupervised`: the kernel starts it once at boot and the
    /// supervisor leaves it alone, so `proc_stop` may stop it for good.
    pub unsupervised: bool,
}

/// The kernel's request that a `start=demand` service start: `demand <name>`.
pub const DEMAND: &str = "demand";

/// The service `text` (`demand <name>`) names, if it is one.
pub fn parse_demand(text: &str) -> Option<&str> {
    let mut words = text.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some(DEMAND), Some(name), None) if valid_name(name) => Some(name),
        _ => None,
    }
}

/// The index among `specs` of the service `demand <name>` asks for, if the
/// supervisor should start it now: the request is the kernel's (`caller` 0),
/// the line says `start=demand`, it is not running, and the user has not
/// stopped it. `Ok(None)` when it already runs or is starting.
pub fn check_demand(
    name: &str,
    specs: &[&ServiceSpec],
    records: &[Record],
    caller: u64,
    starting: &[bool],
) -> Result<Option<usize>, &'static str> {
    if caller != 0 {
        return Err("err only the kernel asks for a service on demand");
    }
    let index = specs
        .iter()
        .position(|s| s.name == name)
        .ok_or("err unknown service")?;
    if specs[index].start != Start::Demand {
        return Err("err not a start=demand service");
    }
    let record = &records[index];
    if record.stopped {
        return Err("err stopped");
    }
    // Past its restart cap a demand does not start it again; only the user's
    // `start` does. While it backs off it is starting, below, so a demand never
    // skips the backoff either.
    if record.gave_up {
        return Err("err gave up");
    }
    match record.pid != 0 || starting[index] {
        true => Ok(None),
        false => Ok(Some(index)),
    }
}

/// A folder a plan line grants with `dir=` (ADR 0011 §12): the kernel puts a
/// startup `Dir` for it in the service's table, on the `fs` route, and lets the
/// service hold `Dir` and `File` handles. The authority is the plan line, which
/// only the kernel and the supervisor write: no capability bit stands for it,
/// and the kernel hard-codes no program name. The folder goes only to the
/// line's own program, its `program=` or else its name ([`Plan::grants`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    /// `/system/pkg`, the package store (ADR 0011 §5).
    Pkg,
    /// `/system/trust`, the cached TUF metadata (ADR 0008 §7): `updated`'s only.
    Trust,
    /// `/system/updates`, staged releases and the `update` snapshot's handle
    /// (ADR 0008 §10): `updated`'s only.
    Updates,
    /// `/system/settings`, the Settings app's files, read only: for a service
    /// that follows a setting (the desktop's appearance and the supervisor's
    /// profiles next). Nothing writes it in v0.1.
    Settings,
    /// `/system/apps`, where comphost keeps each app's saved state: one folder
    /// per app, named for it (P4). comphost is the OS's own host and the app's
    /// wasm never reaches `fs`, so a host only touches its own app's folder.
    Apps,
    /// `/system/state/restart`, read only: the restart window the shell saves
    /// from a Settings click (`kai-updates 1`), `updated`'s alone.
    Restart,
    /// The `models` volume's root, the model store: `updated`'s only, which
    /// imports a signed model into it and points `kai-modelhint 1` at one
    /// (P4 task 10).
    Models,
}

impl Folder {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pkg" => Some(Folder::Pkg),
            "trust" => Some(Folder::Trust),
            "updates" => Some(Folder::Updates),
            "settings" => Some(Folder::Settings),
            "apps" => Some(Folder::Apps),
            "restart" => Some(Folder::Restart),
            "models" => Some(Folder::Models),
            _ => None,
        }
    }

    /// The badge `fs` serves it under, which it looks up itself.
    pub fn badge(self) -> u64 {
        match self {
            Folder::Pkg => kabi::fs::SYSTEM_PKG,
            Folder::Trust => kabi::fs::SYSTEM_TRUST,
            Folder::Updates => kabi::fs::SYSTEM_UPDATES,
            Folder::Settings => kabi::fs::SYSTEM_SETTINGS,
            Folder::Apps => kabi::fs::SYSTEM_APPS,
            Folder::Restart => kabi::fs::SYSTEM_RESTART,
            Folder::Models => kabi::fs::MODELS_ROOT,
        }
    }

    /// Whether its `Dir` only reads: the kernel gives it READ and ENUMERATE, and
    /// `fs` refuses a change through it whatever the handle says.
    pub fn read_only(self) -> bool {
        matches!(self, Folder::Settings | Folder::Restart)
    }

    /// The one service, by name and by program, a line granting it must be, if
    /// it has one: the trust store, the updates folder, the restart window and
    /// the model store are `updated`'s alone (ADR 0008 §10), as the kernel's
    /// UPDATE capability is.
    pub fn only_for(self) -> Option<&'static str> {
        match self {
            Folder::Pkg | Folder::Settings | Folder::Apps => None,
            Folder::Trust | Folder::Updates | Folder::Restart | Folder::Models => Some(UPDATED),
        }
    }
}

/// The update service's program, the only one `dir=trust` and `dir=updates`
/// may go to.
pub const UPDATED: &str = "updated";

/// Boot files a plan line lets its service read with BOOT_READ, from `bootfiles=`
/// (P4 task 1): one folder of the boot bundle, read-only. Like [`Folder`], the
/// authority is the plan line, and the grant goes only to the line's own
/// program ([`Plan::grants`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootFiles {
    /// `pkgrepo/`, the image's signed package repository (ADR 0011 §8).
    Pkgrepo,
}

/// The longest path BOOT_READ takes.
pub const MAX_BOOT_PATH: usize = 160;

impl BootFiles {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pkgrepo" => Some(BootFiles::Pkgrepo),
            _ => None,
        }
    }

    /// The bundle folder it names, with its `/`.
    pub fn prefix(self) -> &'static str {
        match self {
            BootFiles::Pkgrepo => "pkgrepo/",
        }
    }

    /// Whether `path`, relative to the bundle's top, lies beneath this folder:
    /// at most [`MAX_BOOT_PATH`] bytes of `[A-Za-z0-9._-]` names joined by single
    /// `/`s, none of them `.` or `..`, after the folder's prefix.
    pub fn allows(self, path: &str) -> bool {
        let Some(rest) = path.strip_prefix(self.prefix()) else {
            return false;
        };
        path.len() <= MAX_BOOT_PATH
            && rest.split('/').all(|name| {
                !name.is_empty()
                    && name != "."
                    && name != ".."
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            })
    }
}

/// When the supervisor starts a service (P4 task 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Start {
    /// At boot, as every service did before packages.
    #[default]
    Boot,
    /// `start=pkg`: an app the current generation holds. It stays down until the
    /// holder of `/system/pkg` asks with [`Run`], and then runs, and restarts, on
    /// the package version that request names, passed as `version=`.
    Pkg,
    /// `start=demand` (P3 task 12b, app nap): not at boot, but when something
    /// first needs it. A call that waits for it makes the kernel send the
    /// supervisor [`DEMAND`] for it ([`check_demand`]).
    Demand,
}

/// What a plan line grants its service besides capability bits.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    /// Its routes, from `uses=`.
    pub uses: Vec<String>,
    /// Its startup folders, from `dir=`.
    pub dirs: Vec<Folder>,
    /// Its boot files, from `bootfiles=`.
    pub boot: Option<BootFiles>,
    /// Its network destination, from `netdest=`.
    pub netdest: Option<netdest::Dest>,
    /// Its handle to the LLM server's destination, from `netdest=llm`.
    pub llm_dest: bool,
    /// The badge on its route to `llm`, from `agent=`.
    pub agent: Option<u64>,
    /// The MiB a day its `netdest=` may move, from `net_mib=`.
    pub net_mib: Option<u32>,
    /// The routes of [`Grants::uses`] the process may call through but never
    /// pass on (CALL only, no TRANSFER or DUPLICATE): an agent's routes to
    /// `llm` and the shell, whose badge is its identity, and a route to `fetch`'s send role,
    /// which posts. A copy given to another process would carry the badge or
    /// the send with it.
    pub pinned: Vec<String>,
}

/// Names nothing may stop: the shell, the supervisor and the model service (as
/// `agentabi::rows::PROTECTED` says for the agent's rows).
pub const PROTECTED: &[&str] = &["shell", "supervisor", "llm"];

/// The service the supervisor serves, and which a plan line may name in `uses=`
/// although no line lists it.
pub const SUPERVISOR: &str = "supervisor";

/// A request to the supervisor's service, and the [`Proof`] that the user wants
/// it, which the supervisor checks before it acts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    /// `stop <name>`: end the service and keep it down until `start`.
    Stop(&'a str),
    /// `start <name>`: start a stopped service again.
    Start(&'a str),
    /// `restart <name>`: end the service and start it at once, or start it if it
    /// is stopped. One request, since a `ONCE` is used up by one check.
    Restart(&'a str),
    /// `suspend <name>`: the app saves its state and drops its instance
    /// (`app_suspend`, P3 task 11), through the kernel's `ctl suspend`.
    Suspend(&'a str),
    /// `resume <name>`: the app comes back from its saved state (`app_resume`).
    Resume(&'a str),
}

/// The user's yes on a menu: the `ONCE` badge the trusted path gave for menu
/// entry `id` (ADR 0001 D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Once {
    pub badge: u64,
    pub id: u64,
}

/// Why the supervisor may act on a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    /// Nothing: enough only for `start`, which the shell runs without asking.
    None,
    /// `id=<entry>`: a `start` from the shell's log entry `entry`, which a later
    /// undo names.
    Entry(u64),
    /// `once=<badge> id=<entry>`: the menu's Allow.
    Once(Once),
    /// `rule`: a standing rule the caller holds, checked with RULE_CHECK.
    Rule,
    /// `undo id=<entry>`: puts back the `start` the shell made as log entry
    /// `entry`, if nothing has changed the service since.
    Undo(u64),
}

impl<'a> Request<'a> {
    /// `<verb> <name>`, then the words of its [`Proof`].
    pub fn parse(text: &'a str) -> Option<(Self, Proof)> {
        let mut words = text.split_whitespace();
        let request = match (words.next()?, words.next()?) {
            ("stop", name) => Request::Stop(name),
            ("start", name) => Request::Start(name),
            ("restart", name) => Request::Restart(name),
            ("suspend", name) => Request::Suspend(name),
            ("resume", name) => Request::Resume(name),
            _ => return None,
        };
        let number = |word: Option<&str>, key: &str| word?.strip_prefix(key)?.parse().ok();
        let proof = match (words.next(), words.next()) {
            (None, None) => Proof::None,
            (Some("rule"), None) => Proof::Rule,
            (Some("undo"), id) => Proof::Undo(number(id, "id=")?),
            (Some(id), None) => Proof::Entry(number(Some(id), "id=")?),
            (badge, id) => Proof::Once(Once {
                badge: number(badge, "once=")?,
                id: number(id, "id=")?,
            }),
        };
        words.next().is_none().then_some((request, proof))
    }

    pub fn name(self) -> &'a str {
        match self {
            Request::Stop(name)
            | Request::Start(name)
            | Request::Restart(name)
            | Request::Suspend(name)
            | Request::Resume(name) => name,
        }
    }

    /// The call the menu shows for it, in canonical text: `service_stop("x")`.
    /// `None` for a name no call can show.
    pub fn call(self) -> Option<String> {
        let action = match self {
            Request::Stop(_) => "service_stop",
            Request::Start(_) => "service_start",
            Request::Restart(_) => "service_restart",
            Request::Suspend(_) => "app_suspend",
            Request::Resume(_) => "app_resume",
        };
        termcore::once::call_text(action, &[termcore::once::CallArg::Text(self.name())])
    }

    /// The `ONCE` digest of it on menu entry `id`, as the trusted path holds it.
    pub fn digest(self, id: u64) -> Option<[u8; 32]> {
        Some(termcore::once::digest_call(id, &self.call()?))
    }

    /// The request's text, with its proof.
    pub fn encode(self, proof: Proof) -> String {
        let verb = match self {
            Request::Stop(_) => "stop",
            Request::Start(_) => "start",
            Request::Restart(_) => "restart",
            Request::Suspend(_) => "suspend",
            Request::Resume(_) => "resume",
        };
        let name = self.name();
        match proof {
            Proof::None => format!("{verb} {name}"),
            Proof::Entry(id) => format!("{verb} {name} id={id}"),
            Proof::Once(once) => format!("{verb} {name} once={} id={}", once.badge, once.id),
            Proof::Rule => format!("{verb} {name} rule"),
            Proof::Undo(id) => format!("{verb} {name} undo id={id}"),
        }
    }
}

/// Whether `proof` is the kind a request of this verb may carry, before it is
/// checked: a start needs none (it is run-tier and has no menu); a stop or a
/// restart needs a `ONCE` or a rule, and only a stop may be an undo; a suspend
/// or a resume, which a standing instruction fires, a `ONCE` or a rule.
pub fn proof_fits(request: Request, proof: Proof) -> Result<(), &'static str> {
    match (request, proof) {
        (Request::Suspend(_) | Request::Resume(_), Proof::Once(_) | Proof::Rule) => Ok(()),
        (Request::Suspend(_) | Request::Resume(_), _) => Err("err needs ONCE or a rule"),
        (Request::Start(_), Proof::None | Proof::Entry(_)) => Ok(()),
        (Request::Start(_), _) => Err("err a start takes only id="),
        (_, Proof::Once(_) | Proof::Rule) | (Request::Stop(_), Proof::Undo(_)) => Ok(()),
        _ => Err("err needs ONCE"),
    }
}

/// The request that lists the services the supervisor runs, for the shell to
/// tell a service from a plain process (`services`).
pub const SERVICES: &str = "services";

/// A service as `services` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    /// The running instance's pid, or 0 between restarts.
    pub pid: u64,
    /// Stopped by a `stop` request, until `start`.
    pub stopped: bool,
}

/// The answer to `services`: `ok`, then `name=pid` for each service, or
/// `name=stopped`. Protected names are left out: nothing may stop them.
pub fn listing(records: &[Record]) -> String {
    let mut text = String::from("ok");
    for record in records
        .iter()
        .filter(|r| !PROTECTED.contains(&r.name.as_str()))
    {
        match record.stopped {
            true => text.push_str(&format!(" {}=stopped", record.name)),
            false => text.push_str(&format!(" {}={}", record.name, record.pid)),
        }
    }
    text
}

/// Reads a [`listing`]; `None` if it is not one.
pub fn parse_listing(text: &str) -> Option<Vec<Listed>> {
    let mut words = text.split_whitespace();
    if words.next()? != "ok" {
        return None;
    }
    words
        .map(|word| {
            let (name, state) = word.split_once('=')?;
            let (pid, stopped) = match state {
                "stopped" => (0, true),
                pid => (pid.parse().ok()?, false),
            };
            let name = name.to_string();
            Some(Listed { name, pid, stopped })
        })
        .collect()
}

/// What the supervisor does for `request` given its `records`: the record to act
/// on, or the reply that refuses it (`err protected`, `err unknown service`,
/// `err already stopped`, `err not stopped`).
pub fn check(request: Request, records: &[Record]) -> Result<usize, &'static str> {
    check_for(request, records, &[])
}

/// [`check`], knowing each record's [`Start`] (`starts`, by index; a record
/// past its end starts at boot): a `start` of a `start=demand` service that
/// has not started yet starts it, as its first demand would.
pub fn check_for(
    request: Request,
    records: &[Record],
    starts: &[Start],
) -> Result<usize, &'static str> {
    let name = request.name();
    if PROTECTED.contains(&name) {
        return Err("err protected");
    }
    let index = records
        .iter()
        .position(|record| record.name == name)
        .ok_or("err unknown service")?;
    let record = &records[index];
    let waiting = starts.get(index) == Some(&Start::Demand) && record.pid == 0;
    match (request, record.stopped) {
        (Request::Stop(_), true) => Err("err already stopped"),
        // A service the supervisor gave up on starts again on request too, and
        // so does a demand service not started yet.
        (Request::Start(_), false) if !record.gave_up && !waiting => Err("err not stopped"),
        _ => Ok(index),
    }
}

#[cfg(test)]
mod agent_tests;
#[cfg(test)]
mod net_mib_tests;
#[cfg(test)]
mod startup_tests;
#[cfg(test)]
mod tests;
