//! The ring-3 supervisor's decisions, kept free of system calls so they can be
//! tested on the host: reading the service plan, the restart policy (exponential
//! backoff with a restart-intensity cap, as in Erlang/OTP), and the state saved in
//! the kernel-held region so a restarted supervisor carries on where it left off.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::{Limits, cap};

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
    /// Restart policy; the plan may change `max_backoff_ms` and `max_restarts`.
    pub policy: Policy,
    /// From the word `unsupervised`: the kernel starts it once at boot and the
    /// supervisor leaves it alone, so `proc_stop` may stop it for good.
    pub unsupervised: bool,
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
}

impl Folder {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pkg" => Some(Folder::Pkg),
            "trust" => Some(Folder::Trust),
            "updates" => Some(Folder::Updates),
            _ => None,
        }
    }

    /// The badge `fs` serves it under, which it looks up itself.
    pub fn badge(self) -> u64 {
        match self {
            Folder::Pkg => kabi::fs::SYSTEM_PKG,
            Folder::Trust => kabi::fs::SYSTEM_TRUST,
            Folder::Updates => kabi::fs::SYSTEM_UPDATES,
        }
    }

    /// The one service, by name and by program, a line granting it must be, if
    /// it has one: the trust store and the updates folder are `updated`'s alone
    /// (ADR 0008 §10), as the kernel's UPDATE capability is.
    pub fn only_for(self) -> Option<&'static str> {
        match self {
            Folder::Pkg => None,
            Folder::Trust | Folder::Updates => Some(UPDATED),
        }
    }
}

/// The update service's program, the only one `dir=trust` and `dir=updates`
/// may go to.
pub const UPDATED: &str = "updated";

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
            Request::Stop(name) | Request::Start(name) | Request::Restart(name) => name,
        }
    }

    /// The call the menu shows for it, in canonical text: `service_stop("x")`.
    /// `None` for a name no call can show.
    pub fn call(self) -> Option<String> {
        let action = match self {
            Request::Stop(_) => "service_stop",
            Request::Start(_) => "service_start",
            Request::Restart(_) => "service_restart",
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
/// restart needs a `ONCE` or a rule, and only a stop may be an undo.
pub fn proof_fits(request: Request, proof: Proof) -> Result<(), &'static str> {
    match (request, proof) {
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
    let name = request.name();
    if PROTECTED.contains(&name) {
        return Err("err protected");
    }
    let index = records
        .iter()
        .position(|record| record.name == name)
        .ok_or("err unknown service")?;
    let record = &records[index];
    match (request, record.stopped) {
        (Request::Stop(_), true) => Err("err already stopped"),
        // A service the supervisor gave up on starts again on request too.
        (Request::Start(_), false) if !record.gave_up => Err("err not stopped"),
        _ => Ok(index),
    }
}

/// A parsed plan: the services that parsed, and why any other line was skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub services: Vec<ServiceSpec>,
    pub errors: Vec<String>,
}

impl Plan {
    /// The routes (`uses=`) and folders (`dir=`) this plan grants service `name`
    /// running `program`; none for a service it does not list. A line's folders
    /// go only to its own program, its `program=` or else its name: started with
    /// any other, the service is refused, as PUBLISH is, so a spawn that keeps
    /// the name and swaps the program never gets the folder (ADR 0011 §12).
    pub fn grants(&self, name: &str, program: &str) -> Result<(Vec<String>, Vec<Folder>), String> {
        let Some(service) = self.services.iter().find(|s| s.name == name) else {
            return Ok(Default::default());
        };
        if !service.dirs.is_empty() && service.program != program {
            return Err(format!(
                "{name}: its dir= is for program {}, not {program}",
                service.program
            ));
        }
        Ok((service.uses.clone(), service.dirs.clone()))
    }
}

/// Reads the plan: one service per line, `name [program=P] [pages=N] [stack_pages=N]
/// [burst_ms=N] [caps=serve+net+call+policy+component+publish+model+threads+resolve+stop+script]
/// [uses=A+B] [dir=pkg] [backoff_max_ms=N] [max_restarts=N] [unsupervised] [other=args ...]`. The supervisor keys set limits,
/// routes, folders and policy; everything else is passed to the program. A bad line skips only
/// that service, so one typo cannot keep every other service down.
pub fn parse_plan(text: &str) -> Plan {
    let mut plan = Plan::default();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match parse_line(line, &plan.services) {
            Ok(spec) => plan.services.push(spec),
            Err(error) => plan.errors.push(error),
        }
    }
    plan
}

fn parse_line(line: &str, earlier: &[ServiceSpec]) -> Result<ServiceSpec, String> {
    let mut words = line.split_whitespace();
    let name = words.next().unwrap_or_default();
    if !valid_name(name) {
        return Err(format!("bad service name {name:?}"));
    }
    if earlier.iter().any(|s| s.name == name) {
        return Err(format!("service {name} listed twice"));
    }
    let mut limits = Limits {
        max_pages: DEFAULT_PAGES,
        max_burst_ms: DEFAULT_BURST_MS,
        caps: 0,
        stack_pages: 0,
    };
    let mut policy = POLICY;
    let mut args = Vec::new();
    let mut program = name;
    let mut uses = Vec::new();
    let mut dirs: Option<Vec<Folder>> = None;
    let mut unsupervised = false;
    for word in words {
        if word == "unsupervised" {
            unsupervised = true;
            continue;
        }
        match word.split_once('=') {
            Some(("program", value)) => {
                if !valid_name(value) {
                    return Err(format!("{name}: bad program name {value:?}"));
                }
                // The supervisor runs only as the kernel starts it.
                if value == "supervisor" {
                    return Err(format!("{name}: the supervisor cannot run as a service"));
                }
                program = value;
            }
            Some(("pages", value)) => limits.max_pages = number(name, value)?,
            Some(("stack_pages", value)) => {
                limits.stack_pages = number(name, value)?;
                if limits.stack_pages > kabi::layout::MAX_STACK_PAGES {
                    return Err(format!(
                        "{name}: stack_pages over {}",
                        kabi::layout::MAX_STACK_PAGES
                    ));
                }
            }
            Some(("burst_ms", value)) => limits.max_burst_ms = number(name, value)?,
            Some(("caps", value)) => limits.caps = caps(name, value)?,
            Some(("uses", value)) => uses = routes(name, value, earlier)?,
            Some(("dir", value)) => {
                if dirs.is_some() {
                    return Err(format!("{name}: dir= given twice"));
                }
                dirs = Some(folders(name, value)?);
            }
            Some(("backoff_max_ms", value)) => policy.max_backoff_ms = number(name, value)?,
            Some(("max_restarts", value)) => policy.max_restarts = number(name, value)? as usize,
            _ => args.push(word),
        }
    }
    if !uses.is_empty() && limits.caps & cap::CALL == 0 {
        return Err(format!("{name}: uses= needs the call capability"));
    }
    let dirs = dirs.unwrap_or_default();
    if let Some(only) = dirs
        .iter()
        .find_map(|d| d.only_for().filter(|o| *o != name || *o != program))
    {
        return Err(format!(
            "{name}: that dir= is for the service {only} running {only} only"
        ));
    }
    // The kernel starts it with no supervisor to answer for it: no capabilities.
    if unsupervised && (limits.caps != 0 || !uses.is_empty() || !dirs.is_empty()) {
        return Err(format!(
            "{name}: an unsupervised process takes no caps=, uses= or dir="
        ));
    }
    Ok(ServiceSpec {
        name: name.to_string(),
        program: program.to_string(),
        args: args.join(" "),
        limits,
        uses,
        dirs,
        policy,
        unsupervised,
    })
}

/// The folders in `dir=A+B`: each one [`Folder`] names, and each once. Any other
/// name fails the line, so a typo grants nothing.
fn folders(service: &str, value: &str) -> Result<Vec<Folder>, String> {
    let mut dirs: Vec<Folder> = Vec::new();
    for word in value.split('+') {
        let folder =
            Folder::parse(word).ok_or_else(|| format!("{service}: no folder {word:?} for dir="))?;
        if dirs.contains(&folder) {
            return Err(format!("{service}: dir={word} twice"));
        }
        dirs.push(folder);
    }
    Ok(dirs)
}

/// Services nobody may call: the shell takes only the terminal's lines, and the
/// edit service only the kernel's (ADR 0001 D3).
pub const UNCALLABLE: &[&str] = &["shell", "edit"];

/// The routes in `uses=A+B`: each a service listed before this one, so routes
/// form no cycle, never itself, `shell` or `edit`, and each once.
fn routes(service: &str, value: &str, earlier: &[ServiceSpec]) -> Result<Vec<String>, String> {
    let mut uses: Vec<String> = Vec::new();
    for target in value.split('+') {
        if UNCALLABLE.contains(&target) || target == service {
            return Err(format!("{service}: no route may go to {target}"));
        }
        if target != SUPERVISOR && !earlier.iter().any(|s| s.name == target) {
            return Err(format!(
                "{service}: uses {target}, which is not listed before it"
            ));
        }
        if uses.iter().any(|u| u == target) {
            return Err(format!("{service}: uses {target} twice"));
        }
        uses.push(target.to_string());
    }
    Ok(uses)
}

/// A service or program name: letters, digits and '-', at most `MAX_NAME` bytes.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= kabi::MAX_NAME
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn number(service: &str, value: &str) -> Result<u64, String> {
    value
        .parse()
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("{service}: bad number {value:?}"))
}

fn caps(service: &str, value: &str) -> Result<u64, String> {
    value.split('+').try_fold(0, |bits, name| match name {
        "serve" => Ok(bits | cap::SERVE),
        "net" => Ok(bits | cap::NET),
        "call" => Ok(bits | cap::CALL),
        "policy" => Ok(bits | cap::POLICY),
        "component" => Ok(bits | cap::COMPONENT),
        "publish" => Ok(bits | cap::PUBLISH),
        "model" => Ok(bits | cap::MODEL),
        "threads" => Ok(bits | cap::THREADS),
        "resolve" => Ok(bits | cap::RESOLVE),
        "stop" => Ok(bits | cap::STOP),
        "script" => Ok(bits | cap::SCRIPT),
        "partition" => Ok(bits | cap::PARTITION),
        "files" => Ok(bits | cap::FILES),
        "state" => Ok(bits | cap::SYSTEM_STATE),
        "display" => Ok(bits | cap::DISPLAY),
        "input" => Ok(bits | cap::INPUT),
        "term" => Ok(bits | cap::TERM),
        "update" => Ok(bits | cap::UPDATE),
        other => Err(format!("{service}: unknown capability {other:?}")),
    })
}

/// When to restart a service that ended, or whether to stop trying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    RestartAfterMs(u64),
    GiveUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub first_backoff_ms: u64,
    pub max_backoff_ms: u64,
    /// A run at least this long resets the backoff.
    pub healthy_run_ms: u64,
    /// More than `max_restarts` restarts within `window_ms` and the service is given up.
    pub window_ms: u64,
    pub max_restarts: usize,
}

pub const POLICY: Policy = Policy {
    first_backoff_ms: 50,
    max_backoff_ms: 1600,
    healthy_run_ms: 5000,
    window_ms: 60_000,
    max_restarts: 30,
};

/// What the supervisor remembers about a service, saved across its own restarts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    /// Restarts so far, ever.
    pub restarts: u64,
    /// Restarts in a row that ran shorter than `healthy_run_ms`.
    pub quick_failures: u64,
    /// Boot-relative times of recent restarts, in milliseconds, oldest first.
    pub recent: Vec<u64>,
    pub gave_up: bool,
    /// Stopped by a `stop` request: not restarted, even by a new supervisor,
    /// until `start`.
    pub stopped: bool,
    /// The running instance's pid, or 0. Lets a restarted supervisor match exit
    /// records for instances started by its predecessor.
    pub pid: u64,
    /// When the running instance started (boot-relative ms), so a restarted
    /// supervisor can tell how long it ran.
    pub started_ms: u64,
}

impl Record {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Self::default()
        }
    }

    /// The service ended at `now_ms` after running `ran_ms`.
    pub fn on_exit(&mut self, policy: &Policy, now_ms: u64, ran_ms: u64) -> Decision {
        if self.gave_up {
            return Decision::GiveUp;
        }
        self.recent
            .retain(|&at| now_ms.saturating_sub(at) < policy.window_ms);
        if self.recent.len() >= policy.max_restarts {
            self.gave_up = true;
            return Decision::GiveUp;
        }
        self.quick_failures = if ran_ms >= policy.healthy_run_ms {
            0
        } else {
            self.quick_failures + 1
        };
        self.restarts += 1;
        self.recent.push(now_ms);
        let shift = self.quick_failures.saturating_sub(1).min(16);
        let backoff = (policy.first_backoff_ms << shift).min(policy.max_backoff_ms);
        Decision::RestartAfterMs(if self.quick_failures == 0 { 0 } else { backoff })
    }
}

const STATE_MAGIC: &str = "supervisor-state 4";
/// The state before `stopped`, still read so an update keeps the counts.
const STATE_MAGIC_3: &str = "supervisor-state 3";

/// Encodes records for the kernel-held state region.
pub fn encode(records: &[Record]) -> String {
    let mut text = String::from(STATE_MAGIC);
    for record in records {
        text.push('\n');
        text.push_str(&format!(
            "{} {} {} {} {} {} {}",
            record.name,
            record.restarts,
            record.quick_failures,
            u8::from(record.gave_up),
            u8::from(record.stopped),
            record.pid,
            record.started_ms
        ));
        for at in &record.recent {
            text.push_str(&format!(" {at}"));
        }
    }
    text
}

/// Decodes saved records. Anything unreadable yields no records rather than an error:
/// a supervisor must still start when its saved state is damaged.
pub fn decode(bytes: &[u8]) -> Vec<Record> {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    let has_stopped = match lines.next() {
        Some(STATE_MAGIC) => true,
        Some(STATE_MAGIC_3) => false,
        _ => return Vec::new(),
    };
    lines
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?.to_string();
            let restarts = words.next()?.parse().ok()?;
            let quick_failures = words.next()?.parse().ok()?;
            let gave_up = words.next()? == "1";
            let stopped = has_stopped && words.next()? == "1";
            let pid = words.next()?.parse().ok()?;
            let started_ms = words.next()?.parse().ok()?;
            let recent = words
                .map(str::parse)
                .collect::<Result<Vec<u64>, _>>()
                .ok()?;
            Some(Record {
                name,
                restarts,
                quick_failures,
                recent,
                gave_up,
                stopped,
                pid,
                started_ms,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
