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
//! own restarts need none either. `suspend <name>` and `resume <name>` run a
//! standing instruction's `app_suspend` and `app_resume` (P3 task 11): with a
//! rule, checked with RULE_CHECK, the kernel sends the app its `ctl` (APP_CTL)
//! and the app's answer is the reply.
//! `services` lists them, for the shell to tell a service from a process.
//! A `start=demand` service is not started at boot: `demand <name>`, which only
//! the kernel sends, when a call first waits for it, starts it (P3 task 12b).
//! `profile <pid> <class> <pages|->` sets one process's CPU class and page quota
//! for a resource profile (P3 task 12), for the shell alone: PROC_CLASS_SET and
//! PROC_QUOTA_SET need SUPERVISE, and the kernel's floors refuse a protected
//! process. The answer is what the process had, for `undo`.
//! `keep <name> on|off`, also for the shell alone, keeps an app from napping
//! (`supervise::keep`): the flag lives in its record and goes to it as `keep=1`
//! at each start, and to a running app at once as the kernel's
//! `ctl keep-running on|off`.

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
        // A `start=pkg` app waits for the store holder's `run`, and a
        // `start=demand` service for the kernel's `demand`.
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
    // The class each process was last set to by a profile; any other runs normal.
    let mut classes: Vec<(u64, u64)> = Vec::new();
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
            Ok(Event::Request) => serve(&mut services, &mut classes),
            Ok(Event::Timeout) => {}
            Err(error) => {
                println!("wait_child failed: {error:?}");
                sys::sleep_ms(100);
            }
        }
    }
}

/// Takes the waiting request and answers it: `ok`, or why not.
fn serve(services: &mut [Service], classes: &mut Vec<(u64, u64)>) {
    let mut buf = [0u8; 128];
    let mut caller = 0;
    // The request is already queued, so this does not wait.
    let Ok(len) = sys::svc_recv(&mut buf, 1, &mut caller) else {
        return;
    };
    let text = core::str::from_utf8(&buf[..len.min(buf.len())]).unwrap_or("");
    let reply = match supervise::profile::Set::parse(text) {
        Some(set) => set_profile(services, classes, set, caller),
        None => handle(services, text, caller),
    };
    if let Err(error) = sys::svc_reply(reply.as_bytes()) {
        println!("cannot answer {text:?}: {error:?}");
    }
}

fn handle(services: &mut [Service], text: &str, caller: u64) -> String {
    let records: Vec<Record> = services.iter().map(|s| s.record.clone()).collect();
    if text.trim() == supervise::SERVICES {
        return supervise::listing(&records);
    }
    if let Some(name) = supervise::parse_demand(text) {
        return demand(services, &records, name, caller);
    }
    if let Some(run) = supervise::Run::parse(text) {
        return run_package(services, run, caller);
    }
    if let Some(withdraw) = supervise::Run::parse_withdraw(text) {
        return withdraw_package(services, withdraw, caller);
    }
    if let Some((name, on)) = supervise::keep::parse(text) {
        return keep(services, &records, name, on, caller);
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
    let starts: Vec<Start> = services.iter().map(|s| s.spec.start).collect();
    let index = match supervise::check_for(request, &records, &starts) {
        Ok(index) => index,
        Err(why) => return String::from(why),
    };
    let service = &mut services[index];
    let name = service.spec.name.clone();
    if let Request::Suspend(_) | Request::Resume(_) = request {
        return app_ctl(&name, matches!(request, Request::Resume(_)));
    }
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
        Request::Suspend(_) | Request::Resume(_) => {}
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

/// `demand <name>` from the kernel: a call waits for `start=demand` service
/// `name`, so it starts now (P3 task 12b).
fn demand(services: &mut [Service], records: &[Record], name: &str, caller: u64) -> String {
    let specs: Vec<&ServiceSpec> = services.iter().map(|s| &s.spec).collect();
    let starting: Vec<bool> = services.iter().map(|s| s.start_at_ms.is_some()).collect();
    match supervise::check_demand(name, &specs, records, caller, &starting) {
        Ok(Some(index)) => {
            services[index].start_at_ms = Some(now_ms());
            println!("{name} starts on demand");
            String::from("ok")
        }
        Ok(None) => String::from("ok"),
        Err(why) => why.into(),
    }
}

/// `keep <name> on|off` from the shell: the flag goes in the app's record,
/// which passes `keep=1` at each start, and a running app hears it at once.
fn keep(services: &mut [Service], records: &[Record], name: &str, on: bool, caller: u64) -> String {
    let specs: Vec<&ServiceSpec> = services.iter().map(|s| &s.spec).collect();
    let shell = services
        .iter()
        .find(|s| s.spec.name == "shell")
        .and_then(|s| s.pid);
    let index = match supervise::keep::check(name, &specs, records, caller, shell) {
        Ok(index) => index,
        Err(why) => {
            println!(
                "{:?} from pid {caller} refused: {why}",
                supervise::keep::encode(name, on)
            );
            return String::from(why);
        }
    };
    let was = services[index].record.keep;
    services[index].record.keep = on;
    save(services);
    let word = if on { "on" } else { "off" };
    if services[index].pid.is_none() {
        println!("{name} keep-running {word}, from its next start");
        return format!("ok keep-running {word}");
    }
    let verb = if on {
        sys::AppCtl::KeepOn
    } else {
        sys::AppCtl::KeepOff
    };
    let reply = send_ctl(name, verb);
    // A running app that did not take it: the record goes back, so the shell's
    // refusal and the supervisor agree.
    if !reply.starts_with("ok") {
        services[index].record.keep = was;
        save(services);
    }
    reply
}

/// `ctl suspend` (or, `resume`, `ctl resume`) to app `name` through the kernel:
/// the app's own answer, `ok ...` or `err ...`.
fn app_ctl(name: &str, resume: bool) -> String {
    send_ctl(
        name,
        if resume {
            sys::AppCtl::Resume
        } else {
            sys::AppCtl::Suspend
        },
    )
}

/// `verb`'s `ctl` request to app `name` through the kernel (APP_CTL): the
/// app's own answer.
fn send_ctl(name: &str, verb: sys::AppCtl) -> String {
    let mut reply = [0u8; 256];
    match sys::app_ctl(name, verb, &mut reply) {
        Ok(len) => {
            let text = String::from_utf8_lossy(&reply[..len.min(reply.len())]).into_owned();
            println!("{name} {}: {text}", verb.word());
            text
        }
        Err(error) => format!("err cannot reach {name}: {error:?}"),
    }
}

/// `profile <pid> <class> <pages|->`, from the shell only: the class, then the
/// quota; what the process had before. A quota the kernel refuses puts the
/// class back, so a refused request changes nothing.
fn set_profile(
    services: &[Service],
    classes: &mut Vec<(u64, u64)>,
    set: supervise::profile::Set,
    caller: u64,
) -> String {
    let shell = services
        .iter()
        .find(|s| s.spec.name == "shell")
        .and_then(|s| s.pid);
    if shell != Some(caller) {
        return String::from("err only the shell sets profiles");
    }
    let class_before = classes
        .iter()
        .find(|(pid, _)| *pid == set.pid)
        .map_or(0, |(_, class)| *class);
    if let Err(error) = sys::proc_class_set(set.pid, set.class) {
        println!("profile: pid {} class {}: {error:?}", set.pid, set.class);
        return format!("err {error:?}");
    }
    let pages_before = match set.pages {
        None => None,
        Some(pages) => match sys::proc_quota_set(set.pid, pages) {
            Ok(old) => Some(old),
            Err(error) => {
                let _ = sys::proc_class_set(set.pid, class_before);
                println!("profile: pid {} quota {pages}: {error:?}", set.pid);
                return format!("err {error:?}");
            }
        },
    };
    classes.retain(|(pid, _)| *pid != set.pid);
    classes.push((set.pid, set.class));
    supervise::profile::encode_before(supervise::profile::Set {
        pid: set.pid,
        class: class_before,
        pages: pages_before,
    })
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

/// `withdraw <name> version=<label>` from `caller`: the store holder took that
/// package version back out (an undo), so the `start=pkg` app ends if it runs
/// it and waits for `pkg` to name a version again. Only from that holder
/// (`supervise::check_run`); an app on another version keeps running.
fn withdraw_package(services: &mut [Service], withdraw: supervise::Run, caller: u64) -> String {
    let held: Vec<(&ServiceSpec, Option<u64>)> =
        services.iter().map(|s| (&s.spec, s.pid)).collect();
    let index = match supervise::check_run(withdraw, &held, caller) {
        Ok(index) => index,
        Err(why) => {
            println!(
                "{:?} from pid {caller} refused: {why}",
                withdraw.encode_withdraw()
            );
            return String::from(why);
        }
    };
    let service = &mut services[index];
    if service.version.as_deref() != Some(withdraw.version) {
        return String::from("ok");
    }
    let name = service.spec.name.clone();
    service.version = None;
    service.start_at_ms = None;
    if let Some(pid) = service.pid
        && let Err(error) = sys::kill(pid)
    {
        println!("cannot end {name} (pid {pid}): {error:?}");
        return format!("err cannot end it: {error:?}");
    }
    println!(
        "{name} ends: pkg took package version {} back out",
        withdraw.version
    );
    save(services);
    String::from("ok")
}

fn start(service: &mut Service, now: u64) {
    let version = match (service.spec.start, &service.version) {
        (Start::Boot | Start::Demand, _) => String::new(),
        (Start::Pkg, Some(version)) => format!(" version={version}"),
        (Start::Pkg, None) => {
            println!("{} waits for pkg to name its version", service.spec.name);
            service.start_at_ms = None;
            return;
        }
    };
    // The restart count goes to the program, so a restart is visible from outside.
    let keep = match service.record.keep {
        true => format!(" {}", supervise::keep::ARG),
        false => String::new(),
    };
    let args = format!(
        "restart={} {}{version}{keep}",
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
