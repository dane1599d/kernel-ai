//! The supervisor: starts every service in the kernel's plan and restarts the ones
//! that end, with backoff and a restart-intensity cap. Its records live in the
//! kernel-held state region, so when the kernel restarts a dead supervisor the new
//! one carries on with the same counts and re-attaches to services still running.
//!
//! It also serves `supervisor`: `stop <name>` ends a service and keeps it down,
//! across its own restarts too, until `start <name>`, and `restart <name>` ends
//! and starts it (see `supervise::check`). A stop or restart must prove the user
//! wants it (`supervise::Proof`) before it acts: the menu's `ONCE`, checked with
//! ONCE_CHECK against the call shown (ADR 0001 D6); a standing rule the caller
//! holds, checked with RULE_CHECK; or, for a stop, an undo of the start the shell
//! made as that log entry, with nothing changed since. A start needs none. Its
//! own restarts need none either.
//! `services` lists them, for the shell to tell a service from a process.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use kabi::{ExitReason, ExitRecord, SpawnRequest};
use supervise::{Decision, Proof, Record, Request, ServiceSpec, Start};
use user::sys::Event;
use user::{println, sys};

user::entry!(main);

struct Service {
    spec: ServiceSpec,
    record: Record,
    pid: Option<u64>,
    started_ms: u64,
    /// When to start it next, if it is not running.
    start_at_ms: Option<u64>,
    /// Ended by a `restart` request: start it as soon as it is gone, with no
    /// backoff and no restart counted.
    restart_on_exit: bool,
    /// The shell's log entry whose `start` is the latest change to it, for its
    /// undo; cleared by any stop or restart.
    started_by: Option<u64>,
    /// A `start=pkg` service's package version, from the store holder's `run`;
    /// until then it stays down.
    version: Option<String>,
}

fn now_ms() -> u64 {
    sys::time_ns() / 1_000_000
}

fn main() -> i32 {
    let mut buf = vec![0u8; kabi::MAX_STATE];
    let saved = sys::state_load(&mut buf)
        .map(|len| supervise::decode(&buf[..len.min(buf.len())]))
        .unwrap_or_default();
    let mut plan = vec![0u8; kabi::MAX_PLAN];
    let plan = match sys::plan(&mut plan) {
        Ok(len) if len > plan.len() => {
            println!("the plan is {len} bytes, over {}", plan.len());
            return 1;
        }
        Ok(len) => String::from_utf8_lossy(&plan[..len]).into_owned(),
        Err(error) => {
            println!("cannot read the plan: {error:?}");
            return 1;
        }
    };
    let plan = supervise::parse_plan(&plan);
    for error in &plan.errors {
        println!("plan line skipped: {error}");
    }
    // The kernel starts the unsupervised lines itself.
    let specs: Vec<ServiceSpec> = plan
        .services
        .into_iter()
        .filter(|s| !s.unsupervised)
        .collect();
    let now = now_ms();
    let mut services: Vec<Service> = specs
        .into_iter()
        .map(|spec| {
            let record = saved
                .iter()
                .find(|record| record.name == spec.name)
                .cloned()
                .unwrap_or_else(|| Record::new(&spec.name));
            let pid = (record.pid != 0).then_some(record.pid);
            // Known from the saved state for an instance a predecessor started.
            let started_ms = match record.started_ms {
                0 => now,
                at => at,
            };
            Service {
                spec,
                record,
                pid,
                started_ms,
                start_at_ms: None,
                restart_on_exit: false,
                started_by: None,
                version: None,
            }
        })
        .collect();
    // Exits of instances a previous supervisor started but never handled.
    let mut handled = 0;
    while let Ok(Some(exit)) = sys::wait_child(0) {
        on_exit(&mut services, &exit);
        handled += 1;
    }
    // Re-attach to what still runs, and start the rest.
    let running = running_services();
    for service in &mut services {
        service.pid = running
            .iter()
            .find(|(_, name)| *name == service.spec.name)
            .map(|&(pid, _)| pid);
        service.record.pid = service.pid.unwrap_or(0);
        // A `start=pkg` app waits for the store holder's `run`.
        if service.pid.is_none()
            && !service.record.gave_up
            && !service.record.stopped
            && service.start_at_ms.is_none()
            && service.spec.start == Start::Boot
        {
            service.start_at_ms = Some(now);
        }
    }
    if saved.is_empty() {
        println!("starting {} services", services.len());
    } else {
        let attached = services.iter().filter(|s| s.pid.is_some()).count();
        println!(
            "restored saved state for {} services; {handled} exits handled from before; {attached} still running",
            saved.len()
        );
    }
    save(&services);
    loop {
        let now = now_ms();
        let mut started = false;
        for service in &mut services {
            if service.start_at_ms.is_some_and(|at| now >= at) {
                start(service, now);
                started = true;
            }
        }
        if started {
            // Saved with the new pids, so a successor can match their exits.
            save(&services);
        }
        let next_start = services.iter().filter_map(|s| s.start_at_ms).min();
        let timeout = next_start.map_or(1000, |at| at.saturating_sub(now_ms()).clamp(1, 1000));
        match sys::wait_event(timeout) {
            Ok(Event::Exit(exit)) => on_exit(&mut services, &exit),
            Ok(Event::Request) => serve(&mut services),
            Ok(Event::Timeout) => {}
            Err(error) => {
                println!("wait_child failed: {error:?}");
                sys::sleep_ms(100);
            }
        }
    }
}

/// Takes the waiting request and answers it: `ok`, or why not.
fn serve(services: &mut [Service]) {
    let mut buf = [0u8; 128];
    let mut caller = 0;
    // The request is already queued, so this does not wait.
    let Ok(len) = sys::svc_recv(&mut buf, 1, &mut caller) else {
        return;
    };
    let text = core::str::from_utf8(&buf[..len.min(buf.len())]).unwrap_or("");
    let reply = handle(services, text, caller);
    if let Err(error) = sys::svc_reply(reply.as_bytes()) {
        println!("cannot answer {text:?}: {error:?}");
    }
}

fn handle(services: &mut [Service], text: &str, caller: u64) -> String {
    let records: Vec<Record> = services.iter().map(|s| s.record.clone()).collect();
    if text.trim() == supervise::SERVICES {
        return supervise::listing(&records);
    }
    if let Some(run) = supervise::Run::parse(text) {
        return run_package(services, run, caller);
    }
    let Some((request, proof)) = Request::parse(text) else {
        return String::from("err unknown request");
    };
    if let Err(why) = supervise::proof_fits(request, proof) {
        return String::from(why);
    }
    // Checked first, so a request the user did not allow learns nothing more.
    if let Err(why) = check_proof(request, proof, caller) {
        println!("{text:?} refused: {why}");
        return why;
    }
    let index = match supervise::check(request, &records) {
        Ok(index) => index,
        Err(why) => return String::from(why),
    };
    let service = &mut services[index];
    let name = service.spec.name.clone();
    if let Proof::Undo(id) = proof
        && service.started_by != Some(id)
    {
        return String::from("err not that entry's start");
    }
    service.started_by = match (request, proof) {
        (Request::Start(_), Proof::Entry(id)) => Some(id),
        _ => None,
    };
    match request {
        Request::Stop(_) => {
            service.record.stopped = true;
            service.start_at_ms = None;
            if let Some(pid) = service.pid
                && let Err(error) = sys::kill(pid)
            {
                println!("cannot stop {name} (pid {pid}): {error:?}");
            }
            println!("{name} stopped on request");
        }
        Request::Start(_) => {
            service.record.stopped = false;
            service.record.gave_up = false;
            service.start_at_ms = Some(now_ms());
            println!("{name} started on request");
        }
        Request::Restart(_) => {
            service.record.stopped = false;
            service.record.gave_up = false;
            match service.pid {
                Some(pid) => {
                    service.restart_on_exit = true;
                    if let Err(error) = sys::kill(pid) {
                        service.restart_on_exit = false;
                        println!("cannot restart {name} (pid {pid}): {error:?}");
                        return format!("err cannot stop it: {error:?}");
                    }
                }
                None => service.start_at_ms = Some(now_ms()),
            }
            println!("{name} restarted on request");
        }
    }
    save(services);
    String::from("ok")
}

/// A `ONCE` or a rule, checked through the kernel; any other proof needs no call.
fn check_proof(request: Request, proof: Proof, caller: u64) -> Result<(), String> {
    let call = request
        .call()
        .ok_or_else(|| String::from("err unknown service"))?;
    match proof {
        Proof::Once(once) => {
            let digest = request
                .digest(once.id)
                .ok_or_else(|| String::from("err unknown service"))?;
            // Bound to the caller: a process that only saw the badge cannot spend it.
            sys::once_check_for(caller, once.badge, &digest)
                .map_err(|error| format!("err ONCE {error:?}"))
        }
        Proof::Rule => {
            sys::rule_check(caller, &call).map_err(|error| format!("err rule {error:?}"))
        }
        Proof::None | Proof::Entry(_) | Proof::Undo(_) => Ok(()),
    }
}

/// Services the kernel reports running, as (pid, name).
fn running_services() -> Vec<(u64, String)> {
    let mut buf = vec![0u8; 4096];
    let Ok(len) = sys::list(&mut buf) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&buf[..len.min(buf.len())])
        .lines()
        .filter_map(|line| {
            let (pid, name) = line.split_once(' ')?;
            Some((pid.parse().ok()?, String::from(name)))
        })
        .collect()
}

/// `run <name> version=<label>` from `caller`: a `start=pkg` app on the package
/// version the store holder added, and only from that holder
/// (`supervise::check_run`). A running app keeps its version: swapping one is
/// comphost's, under probation.
fn run_package(services: &mut [Service], run: supervise::Run, caller: u64) -> String {
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
        return match service.version.as_deref() == Some(run.version) {
            true => String::from("ok"),
            false => String::from(supervise::RUNS_ANOTHER),
        };
    }
    service.version = Some(String::from(run.version));
    service.record.stopped = false;
    service.record.gave_up = false;
    service.start_at_ms = Some(now_ms());
    println!("{name} starts on package version {} for pkg", run.version);
    save(services);
    String::from("ok")
}

fn start(service: &mut Service, now: u64) {
    let version = match (service.spec.start, &service.version) {
        (Start::Boot, _) => String::new(),
        (Start::Pkg, Some(version)) => format!(" version={version}"),
        (Start::Pkg, None) => {
            println!("{} waits for pkg to name its version", service.spec.name);
            service.start_at_ms = None;
            return;
        }
    };
    // The restart count goes to the program, so a restart is visible from outside.
    let args = format!(
        "restart={} {}{version}",
        service.record.restarts, service.spec.args
    );
    let name = service.spec.name.as_bytes();
    let program = service.spec.program.as_bytes();
    let request = SpawnRequest {
        name_ptr: name.as_ptr() as u64,
        name_len: name.len() as u64,
        args_ptr: args.as_ptr() as u64,
        args_len: args.len() as u64,
        limits: service.spec.limits,
        program_ptr: program.as_ptr() as u64,
        program_len: program.len() as u64,
    };
    match sys::spawn(&request) {
        Ok(pid) => {
            service.pid = Some(pid);
            service.record.pid = pid;
            service.started_ms = now;
            service.record.started_ms = now;
            service.start_at_ms = None;
        }
        Err(error) => {
            println!("cannot start {}: {error:?}", service.spec.name);
            service.start_at_ms = Some(now + 1000);
        }
    }
}

fn on_exit(services: &mut [Service], exit: &ExitRecord) {
    let Some(service) = services.iter_mut().find(|s| s.pid == Some(exit.pid)) else {
        println!("pid {} ({}) ended but is not ours", exit.pid, exit.name());
        return;
    };
    service.pid = None;
    service.record.pid = 0;
    service.record.started_ms = 0;
    // Whatever happens next, the shell's start is no longer the latest change.
    service.started_by = None;
    let now = now_ms();
    let reason = ExitReason::from_raw(exit.reason).map_or("ended", ExitReason::name);
    let name = &service.spec.name;
    if service.restart_on_exit {
        service.restart_on_exit = false;
        println!("{name} {reason}; restarting on request");
        service.start_at_ms = Some(now);
        save(services);
        return;
    }
    if service.record.stopped {
        println!("{name} {reason}; stopped, so not restarted");
        save(services);
        return;
    }
    match service.record.on_exit(
        &service.spec.policy,
        now,
        now.saturating_sub(service.started_ms),
    ) {
        Decision::RestartAfterMs(delay) => {
            println!(
                "{name} {reason} ({:#x}); restart {} in {delay} ms",
                exit.detail, service.record.restarts
            );
            service.start_at_ms = Some(now + delay);
        }
        Decision::GiveUp => println!("{name} {reason}; too many restarts, giving up"),
    }
    save(services);
}

fn save(services: &[Service]) {
    let records: Vec<Record> = services.iter().map(|s| s.record.clone()).collect();
    let state = supervise::encode(&records);
    if let Err(error) = sys::state_save(state.as_bytes()) {
        println!("cannot save state: {error:?}");
    }
}
