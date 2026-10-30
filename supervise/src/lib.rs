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
    /// The boot files it may read with BOOT_READ, from `bootfiles=`.
    pub boot: Option<BootFiles>,
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
    /// that follows a setting (`updated`'s restart window; the desktop's
    /// appearance and the supervisor's profiles next).
    Settings,
    /// `/system/apps`, where comphost keeps each app's saved state: one folder
    /// per app, named for it (P4). comphost is the OS's own host and the app's
    /// wasm never reaches `fs`, so a host only touches its own app's folder.
    Apps,
}

impl Folder {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pkg" => Some(Folder::Pkg),
            "trust" => Some(Folder::Trust),
            "updates" => Some(Folder::Updates),
            "settings" => Some(Folder::Settings),
            "apps" => Some(Folder::Apps),
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
        }
    }

    /// Whether its `Dir` only reads: the kernel gives it READ and ENUMERATE, and
    /// `fs` refuses a change through it whatever the handle says.
    pub fn read_only(self) -> bool {
        matches!(self, Folder::Settings)
    }

    /// The one service, by name and by program, a line granting it must be, if
    /// it has one: the trust store and the updates folder are `updated`'s alone
    /// (ADR 0008 §10), as the kernel's UPDATE capability is.
    pub fn only_for(self) -> Option<&'static str> {
        match self {
            Folder::Pkg | Folder::Settings | Folder::Apps => None,
            Folder::Trust | Folder::Updates => Some(UPDATED),
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

/// The index among `services` (each spec with its running pid) of the service
/// `run` names, if `caller` may ask for it: the running instance of a service
/// whose plan line grants `/system/pkg` (`dir=pkg`), and only for a `start=pkg`
/// service. The kernel checks the holder's program when it spawns it.
pub fn check_run(
    run: Run,
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
        .position(|(spec, _)| spec.name == run.name)
        .ok_or("err unknown service")?;
    match services[index].0.start {
        Start::Pkg => Ok(index),
        Start::Boot | Start::Demand => Err(NOT_FROM_PKG),
    }
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

/// A parsed plan: the services that parsed, and why any other line was skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub services: Vec<ServiceSpec>,
    pub errors: Vec<String>,
}

impl Plan {
    /// The routes (`uses=`), folders (`dir=`) and boot files (`bootfiles=`) this plan
    /// grants service `name` running `program`; none for a service it does not
    /// list. A line's folders and boot files go only to its own program, its
    /// `program=` or else its name: started with any other, the service is
    /// refused, as PUBLISH is, so a spawn that keeps the name and swaps the
    /// program never gets them (ADR 0011 §12).
    pub fn grants(&self, name: &str, program: &str) -> Result<Grants, String> {
        let Some(service) = self.services.iter().find(|s| s.name == name) else {
            return Ok(Grants::default());
        };
        let bound = [
            (!service.dirs.is_empty(), "dir="),
            (service.boot.is_some(), "bootfiles="),
        ];
        if let Some((_, key)) = bound.iter().find(|(held, _)| *held)
            && service.program != program
        {
            return Err(format!(
                "{name}: its {key} is for program {}, not {program}",
                service.program
            ));
        }
        Ok(Grants {
            uses: service.uses.clone(),
            dirs: service.dirs.clone(),
            boot: service.boot,
        })
    }
}

/// Reads the plan: one service per line, `name [program=P] [pages=N] [stack_pages=N]
/// [burst_ms=N] [caps=serve+net+call+policy+component+publish+model+threads+resolve+stop+script]
/// [uses=A+B] [dir=pkg] [bootfiles=pkgrepo] [start=pkg|demand] [backoff_max_ms=N] [max_restarts=N] [unsupervised] [other=args ...]`. The supervisor keys set limits,
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
    let mut boot: Option<BootFiles> = None;
    let mut start: Option<Start> = None;
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
            Some(("bootfiles", value)) => {
                if boot.is_some() {
                    return Err(format!("{name}: bootfiles= given twice"));
                }
                let files = BootFiles::parse(value)
                    .ok_or_else(|| format!("{name}: no boot files {value:?} for bootfiles="))?;
                boot = Some(files);
            }
            Some(("start", value)) => {
                if start.is_some() {
                    return Err(format!("{name}: start= given twice"));
                }
                start = Some(match value {
                    "pkg" => Start::Pkg,
                    "demand" => Start::Demand,
                    _ => {
                        return Err(format!(
                            "{name}: start= is pkg, demand or not given, not {value:?}"
                        ));
                    }
                });
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
    if unsupervised && (limits.caps != 0 || !uses.is_empty() || !dirs.is_empty() || boot.is_some())
    {
        return Err(format!(
            "{name}: an unsupervised process takes no caps=, uses=, dir= or bootfiles="
        ));
    }
    let start = start.unwrap_or_default();
    if unsupervised && start != Start::Boot {
        return Err(format!("{name}: an unsupervised process takes no start="));
    }
    // A package's version comes from `pkg`'s request, never the plan.
    if start == Start::Pkg && args.iter().any(|a| a.starts_with("version=")) {
        return Err(format!("{name}: a start=pkg line names no version="));
    }
    Ok(ServiceSpec {
        name: name.to_string(),
        program: program.to_string(),
        args: args.join(" "),
        limits,
        uses,
        dirs,
        boot,
        start,
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
        "ask" => Ok(bits | cap::ASK),
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
    /// Kept running by the user's switch ([`keep`]): an app that never naps,
    /// passed `keep=1` at each start.
    pub keep: bool,
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

const STATE_MAGIC: &str = "supervisor-state 5";
/// The state before `keep`, still read so an update keeps the counts.
const STATE_MAGIC_4: &str = "supervisor-state 4";
/// The state before `stopped`, still read too.
const STATE_MAGIC_3: &str = "supervisor-state 3";

/// Encodes records for the kernel-held state region.
pub fn encode(records: &[Record]) -> String {
    let mut text = String::from(STATE_MAGIC);
    for record in records {
        text.push('\n');
        text.push_str(&format!(
            "{} {} {} {} {} {} {} {}",
            record.name,
            record.restarts,
            record.quick_failures,
            u8::from(record.gave_up),
            u8::from(record.stopped),
            u8::from(record.keep),
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
    let (has_stopped, has_keep) = match lines.next() {
        Some(STATE_MAGIC) => (true, true),
        Some(STATE_MAGIC_4) => (true, false),
        Some(STATE_MAGIC_3) => (false, false),
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
            let keep = has_keep && words.next()? == "1";
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
                keep,
                pid,
                started_ms,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
