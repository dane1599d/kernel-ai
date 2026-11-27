//! The service plan: `/system/services`, one service a line, read into
//! [`ServiceSpec`]s, with why any other line was skipped.

use crate::*;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kabi::cap;

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
            (service.netdest.is_some() || service.llm_dest, "netdest="),
            (service.agent.is_some(), "agent="),
        ];
        if let Some((_, key)) = bound.iter().find(|(held, _)| *held)
            && service.program != program
        {
            return Err(format!(
                "{name}: its {key} is for program {}, not {program}",
                service.program
            ));
        }
        let pinned = service
            .uses
            .iter()
            .filter(|used| {
                (service.agent.is_some() && kabi::handle::agent::BADGED.contains(&used.as_str()))
                    || sends(&self.services, used)
            })
            .cloned()
            .collect();
        Ok(Grants {
            pinned,
            uses: service.uses.clone(),
            dirs: service.dirs.clone(),
            boot: service.boot,
            netdest: service.netdest.clone(),
            llm_dest: service.llm_dest,
            agent: service.agent,
            net_mib: service.net_mib,
        })
    }
}

/// Reads the plan: one service per line, `name [program=P] [pages=N] [stack_pages=N]
/// [burst_ms=N] [caps=serve+net+call+policy+component+publish+model+threads+stop+script]
/// [uses=A+B] [dir=pkg] [bootfiles=pkgrepo] [netdest=proto:tcp,hosts:example.org|llm] [agent=kai] [start=pkg|demand] [backoff_max_ms=N] [max_restarts=N] [unsupervised] [other=args ...]`. The supervisor keys set limits,
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

pub(crate) fn parse_line(line: &str, earlier: &[ServiceSpec]) -> Result<ServiceSpec, String> {
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
    let mut netdest: Option<netdest::Dest> = None;
    let mut llm_dest = false;
    // The agent= word, checked once the line's program is final: a later
    // program= must not swap the binary its badge goes to.
    let mut agent_word: Option<&str> = None;
    let mut net_mib: Option<u32> = None;
    let mut netdest_word = None;
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
            Some(("netdest", value)) => {
                if netdest.is_some() || llm_dest {
                    return Err(format!("{name}: netdest= given twice"));
                }
                if value == LLM_DEST {
                    llm_dest = true;
                    continue;
                }
                let dest = netdest::Dest::parse(value)
                    .map_err(|why| format!("{name}: netdest={value} is refused: {why}"))?;
                netdest_word = Some(word);
                netdest = Some(dest);
            }
            Some(("agent", value)) => {
                if agent_word.is_some() {
                    return Err(format!("{name}: agent= given twice"));
                }
                agent_word = Some(value);
            }
            Some(("net_mib", value)) => {
                if net_mib.is_some() {
                    return Err(format!("{name}: net_mib= given twice"));
                }
                let mib = value
                    .parse::<u32>()
                    .ok()
                    .filter(|mib| (1..=MAX_NET_MIB).contains(mib))
                    .ok_or_else(|| {
                        format!("{name}: net_mib={value} is not 1 to {MAX_NET_MIB} MiB a day")
                    })?;
                net_mib = Some(mib);
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
    if unsupervised
        && (limits.caps != 0
            || !uses.is_empty()
            || !dirs.is_empty()
            || boot.is_some()
            || netdest.is_some()
            || llm_dest
            || agent_word.is_some())
    {
        return Err(format!(
            "{name}: an unsupervised process takes no caps=, uses=, dir=, bootfiles=, netdest= or agent="
        ));
    }
    // An agent's badge rides on its route to `llm`: a line with none has
    // nowhere to carry it.
    let agent = agent_word
        .map(|value| agent_badge(name, program, value))
        .transpose()?;
    if agent.is_none() && uses.iter().any(|used| used == AGENTS_CALL) {
        return Err(format!("{name}: no route may go to {AGENTS_CALL}"));
    }
    if agent.is_some() && !uses.iter().any(|used| used == kabi::handle::agent::SERVICE) {
        return Err(format!(
            "{name}: agent= needs a route to {} in uses=",
            kabi::handle::agent::SERVICE
        ));
    }
    if let Some(dest) = &netdest {
        any_host_line(name, program, dest)?;
    }
    // The bytes a day are counted on the line's own destination: with none,
    // there is nothing to count them on.
    if net_mib.is_some() && netdest.is_none() {
        return Err(format!("{name}: net_mib= needs a netdest="));
    }
    if llm_dest {
        llm_dest_line(name, program, limits.caps)?;
    }
    // comphost is told its line's destination too: it cannot read one back
    // from the handle, and names it to the shell as the app's ceiling (P5
    // task 13). The kernel holds it to the handle whatever it says. Other
    // programs are not told: none reads it, and a long host list would take
    // room their arguments need under `kabi::MAX_ARGS` (`dlcheck`'s did).
    if program == NETDEST_ARG_PROGRAM {
        args.extend(netdest_word);
    }
    let start = start.unwrap_or_default();
    if unsupervised && start != Start::Boot {
        return Err(format!("{name}: an unsupervised process takes no start="));
    }
    // A package's version comes from `pkg`'s request, never the plan.
    if start == Start::Pkg && args.iter().any(|a| a.starts_with("version=")) {
        return Err(format!("{name}: a start=pkg line names no version="));
    }
    // `fetch` in its send role makes POSTs: only the shell may reach it, its
    // line named `shell` and running `shell`, as DESKTOP's and TERM's one
    // holders are held to name and program.
    if !(name == "shell" && program == "shell")
        && let Some(sender) = uses.iter().find(|used| sends(earlier, used))
    {
        return Err(format!(
            "{name}: only the shell may use {sender}, which sends"
        ));
    }
    // POLICY is the person's: the scheduler's controls need it, and `fs` gives
    // its holder the person's share of the space reserve (PART_PEER_CAPS). Only
    // the shell's line holds it, named `shell` and running `shell`, so no
    // other line, a package's app included, can pass for the person.
    if limits.caps & cap::POLICY != 0 && !(name == "shell" && program == "shell") {
        return Err(format!("{name}: only the shell may hold policy"));
    }
    let spec = ServiceSpec {
        name: name.to_string(),
        program: program.to_string(),
        args: args.join(" "),
        limits,
        uses,
        dirs,
        boot,
        netdest,
        llm_dest,
        agent,
        net_mib,
        start,
        policy,
        unsupervised,
    };
    let held = spec.startup_handles();
    if held > kabi::handle::MAX_STARTUP {
        return Err(format!(
            "{name}: {held} startup handles, more than the {} a process is given",
            kabi::handle::MAX_STARTUP
        ));
    }
    Ok(spec)
}

/// The badge of `agent=value` on the line `name` running `program`: a built-in
/// agent's (`kabi::handle::agent::BUILTIN`), on its own program only, and on
/// the shell's line for `kai`. A packaged agent's badge is the roster's, minted
/// when it is installed, and is not named in a plan.
pub(crate) fn agent_badge(name: &str, program: &str, value: &str) -> Result<u64, String> {
    let (badge, runs) = kabi::handle::agent::builtin(value)
        .ok_or_else(|| format!("{name}: agent={value} is not a built-in agent"))?;
    let named = match value {
        "kai" => name == "shell",
        _ => name == value,
    };
    if program != runs || !named {
        return Err(format!(
            "{name}: agent={value} is for the line {} running {runs}",
            if value == "kai" { "shell" } else { value }
        ));
    }
    Ok(badge)
}

/// Whether the service `name`, planned before, is `fetch` in its send role
/// (`kabi::plan::SEND`).
pub(crate) fn sends(earlier: &[ServiceSpec], name: &str) -> bool {
    let role = format!("role={}", kabi::plan::SEND_ROLE);
    earlier
        .iter()
        .any(|s| s.name == name && s.args.split(' ').any(|word| word == role))
}

impl ServiceSpec {
    /// How many startup handles the kernel gives this line's process, in
    /// STARTUP's order: its caps' (`kabi::cap::STARTUP_HANDLES`), FILES's and
    /// SYSTEM_STATE's `Dir`s and one a `dir=` folder, a route a `uses=`
    /// service (with CALL), and its `netdest=` and `netdest=llm` handles last.
    /// STARTUP lists at most `kabi::handle::MAX_STARTUP`, so a line with more
    /// is refused here and by the kernel: its last handles, its network
    /// ceiling among them, would be lost without a word.
    pub fn startup_handles(&self) -> usize {
        let caps = self.limits.caps;
        let dirs = usize::from(caps & cap::FILES != 0)
            + usize::from(caps & cap::SYSTEM_STATE != 0)
            + self.dirs.len();
        let routes = match caps & cap::CALL != 0 {
            true => self.uses.len(),
            false => 0,
        };
        let llm = self.llm_dest && caps & cap::NET != 0;
        cap::startup_handles(caps)
            + dirs
            + routes
            + usize::from(self.netdest.is_some())
            + usize::from(llm)
    }
}

/// `netdest=llm`: the LLM server's destination, for the line that runs the
/// model service's client of it (K, after #756: `caps=net` alone no longer
/// carries it).
pub const LLM_DEST: &str = "llm";

/// The one plan line `netdest=llm` may sit on: the model service's own.
pub const LLM_DEST_SERVICE: &str = "llm";

/// `netdest=llm` only on the `llm` line running `llm`, and only with `net` in
/// its ceiling, so NET bounds the LLM's handle as it bounds a moved-in `NetDest`
/// (the manager's review of #779). Any other line with it fails.
pub(crate) fn llm_dest_line(name: &str, program: &str, caps: u64) -> Result<(), String> {
    if name != LLM_DEST_SERVICE || program != LLM_DEST_SERVICE {
        return Err(format!(
            "{name}: netdest={LLM_DEST} is for the {LLM_DEST_SERVICE} line alone"
        ));
    }
    if caps & cap::NET == 0 {
        return Err(format!("{name}: netdest={LLM_DEST} needs caps=net"));
    }
    Ok(())
}

/// The most MiB a day `net_mib=` may give: an agent manifest's most
/// (`kaiagent::MAX_NET_MIB_PER_DAY`, which a test holds this to).
pub const MAX_NET_MIB: u32 = 512;

/// The one program told its line's `netdest=` as an argument.
pub const NETDEST_ARG_PROGRAM: &str = "comphost";

/// The one plan line `netdest=...,hosts:any` may sit on: the shell's (the
/// founder, 2026-11-09: Kai may reach any website the user names).
pub const ANY_HOST_SERVICE: &str = "shell";
/// The one port a `hosts:any` destination names: HTTPS.
pub const ANY_HOST_PORT: u16 = 443;

/// `hosts:any` is a ceiling for the shell alone, which narrows it to the one
/// host consent grounded on each call: on the `shell` line running `shell`,
/// TCP to port 443 only, and no literal addresses. Any other line with it
/// fails, so no app's or package's line can name every host.
pub(crate) fn any_host_line(name: &str, program: &str, dest: &netdest::Dest) -> Result<(), String> {
    if !dest.any_host {
        return Ok(());
    }
    if name != ANY_HOST_SERVICE || program != ANY_HOST_SERVICE {
        return Err(format!(
            "{name}: hosts:any is for the {ANY_HOST_SERVICE} line alone"
        ));
    }
    let https_only = dest.protocol == netdest::Protocol::Tcp
        && dest.addrs == netdest::AddrRange::NoAddrs
        && dest.ports == netdest::PortRange::Port(ANY_HOST_PORT);
    match https_only {
        true => Ok(()),
        false => Err(format!(
            "{name}: hosts:any is tcp to port {ANY_HOST_PORT} only, with no addrs"
        )),
    }
}

/// The folders in `dir=A+B`: each one [`Folder`] names, and each once. Any other
/// name fails the line, so a typo grants nothing.
pub(crate) fn folders(service: &str, value: &str) -> Result<Vec<Folder>, String> {
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
/// edit service only the kernel's (ADR 0001 D3). An agent's line is the one
/// exception for the shell ([`AGENTS_CALL`]).
pub const UNCALLABLE: &[&str] = &["shell", "edit"];

/// The uncallable service an `agent=` line may route to: its route carries the
/// agent's badge and is pinned, and the shell admits an agent's step by that
/// badge alone (ADR 0013 section 5).
pub const AGENTS_CALL: &str = "shell";

/// The routes in `uses=A+B`: each a service listed before this one, so routes
/// form no cycle, never itself or `edit`, `shell` only from an agent's line
/// (checked once the line is read), and each once.
pub(crate) fn routes(
    service: &str,
    value: &str,
    earlier: &[ServiceSpec],
) -> Result<Vec<String>, String> {
    let mut uses: Vec<String> = Vec::new();
    for target in value.split('+') {
        if (UNCALLABLE.contains(&target) && target != AGENTS_CALL) || target == service {
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
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= kabi::MAX_NAME
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub(crate) fn number(service: &str, value: &str) -> Result<u64, String> {
    value
        .parse()
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("{service}: bad number {value:?}"))
}

pub(crate) fn caps(service: &str, value: &str) -> Result<u64, String> {
    value.split('+').try_fold(0, |bits, name| match name {
        "serve" => Ok(bits | cap::SERVE),
        "net" => Ok(bits | cap::NET),
        "call" => Ok(bits | cap::CALL),
        "policy" => Ok(bits | cap::POLICY),
        "component" => Ok(bits | cap::COMPONENT),
        "publish" => Ok(bits | cap::PUBLISH),
        "model" => Ok(bits | cap::MODEL),
        "threads" => Ok(bits | cap::THREADS),
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
        "edit" => Ok(bits | cap::EDIT),
        "power" => Ok(bits | cap::POWER),
        other => Err(format!("{service}: unknown capability {other:?}")),
    })
}
