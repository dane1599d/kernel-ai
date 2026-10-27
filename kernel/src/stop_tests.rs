//! `test=proc-stop` (P1 tasks 14 and 15, lane K): the two ways to stop a process,
//! each only with the user's yes (ADR 0001 D6).
//! - PROC_STOP_ONCE, for a Job handle with STOP (only the shell has one) and a
//!   `ONCE` the trusted path minted into the shell's table: it stops the plan's
//!   `unsupervised` `ticker` for good. A badge the shell does not hold, a `ONCE`
//!   for another call or entry, a used one and one whose request ended are
//!   refused, and so are llm, the shell, the supervised `spin` and a name nothing
//!   has. Plain PROC_STOP is the supervisor's: the shell may not make it.
//! - With badge 0, a standing rule the shell holds stands in for the `ONCE`: a
//!   rule for this target covers that call only, one for every target the action,
//!   and neither once `rules_remove` revoked its node.
//! - The supervisor's `stop` and `restart` requests carry `once=<badge>
//!   id=<entry>`, checked against the menu's call (none, a used one and one for
//!   another call are refused), or `rule`; a `start` needs nothing. `spin` stays
//!   down past its restart backoff until `start`; `stop spin undo id=<entry>` stops
//!   it only after that entry's start; `restart` brings it back as a new process.

use crate::handle::grant as grants;
use crate::process::{self, supervision};
use crate::{allow, handle, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use captable::grant::GrantRef;
use kabi::Error;
use kabi::handle::{Kind, Rights};
use supervise::{Once, Proof, Request};
use termcore::menu::Answer;

/// The plan lines this case adds to the shell and llm.
pub const PLAN: &str = "spin program=spinner burst_ms=3600000\n\
     ticker program=spinner burst_ms=3600000 unsupervised";
const SERVICE: &str = "spin";
const UNSUPERVISED: &str = "ticker";
const START_WAIT_MS: u64 = 60_000;
const STOP_WAIT_MS: u64 = 5_000;
/// Longer than the supervisor's first restart backoff, so a restart would show.
const STAYS_DOWN_MS: u64 = 1_000;
/// Menu entry ids for this case's `ONCE`s; any will do, as each has its own node.
const FIRST_ID: u64 = 9_000;

pub fn check() -> Result<(), &'static str> {
    wait_within(START_WAIT_MS, || {
        ipc::server_of("shell").is_some() && running(SERVICE) && running(UNSUPERVISED)
    })
    .map_err(|_| "proc-stop: the shell, spin and ticker did not all come up")?;
    let shell = ipc::server_of("shell")
        .and_then(|pid| {
            process::with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
        })
        .ok_or("proc-stop: the shell has no slot")?;
    let mut ids = FIRST_ID..;
    kernel_side(shell, &mut ids)?;
    wait_within(STOP_WAIT_MS, || !running(UNSUPERVISED))
        .map_err(|_| "proc-stop: ticker did not end")?;
    supervisor_side(shell, &mut ids)?;
    log!(
        "proc-stop: PROC_STOP_ONCE and the supervisor's stop, start and restart ran only with their ONCE and kept to their rules"
    );
    Ok(())
}

/// PROC_STOP_ONCE as the shell meets it.
fn kernel_side(shell: usize, ids: &mut impl Iterator<Item = u64>) -> Result<(), &'static str> {
    if handle::need(shell, kabi::cap::SUPERVISE).is_ok() {
        return Err("proc-stop: the shell may make a plain PROC_STOP");
    }
    let stop = |name: &str, badge, id| supervision::stop_once(shell, name, badge, id);
    rules(shell, ids, &stop)?;
    // No ONCE the shell holds, then ONCEs for another call and another entry.
    expect(
        "no ONCE",
        stop(UNSUPERVISED, 0xdead_beef, 1),
        Err(Error::NotFound),
    )?;
    let id = ids.next().unwrap_or_default();
    let (badge, request) = once(shell, id, &process_stop(SERVICE))?;
    expect(
        "another call",
        stop(UNSUPERVISED, badge, id),
        Err(Error::Invalid),
    )?;
    expect(
        "another entry",
        stop(UNSUPERVISED, badge, id + 1),
        Err(Error::Invalid),
    )?;
    end(request);
    // A server checking for another process: that process must hold the ONCE.
    let id = ids.next().unwrap_or_default();
    let (badge, request) = once(shell, id, &process_stop(UNSUPERVISED))?;
    let digest = termcore::once::digest_call(id, &process_stop(UNSUPERVISED));
    let llm = process::named("llm").first().map_or(0, |&(pid, _)| pid);
    if grants::spend_once_for(llm, badge, &digest) != Err(Error::NotFound) {
        return Err("proc-stop: a ONCE was spent for a process that does not hold it");
    }
    end(request);
    // A request that ended takes its ONCE with it.
    let id = ids.next().unwrap_or_default();
    let (badge, request) = once(shell, id, &process_stop(UNSUPERVISED))?;
    end(request);
    expect(
        "an ended request",
        stop(UNSUPERVISED, badge, id),
        Err(Error::Revoked),
    )?;
    // The right ONCE still keeps PROC_STOP's rules.
    for (name, want) in [
        ("llm", Err(Error::Permission)),
        ("shell", Err(Error::Permission)),
        ("supervisor", Err(Error::Permission)),
        (SERVICE, Err(Error::Invalid)),
        ("nobody", Err(Error::NotFound)),
        (UNSUPERVISED, Ok(1)),
    ] {
        let id = ids.next().unwrap_or_default();
        let (badge, request) = once(shell, id, &process_stop(name))?;
        expect(name, stop(name, badge, id), want)?;
        // Used up by its one check, whatever the stop said.
        expect("a used ONCE", stop(name, badge, id), Err(Error::NotFound))?;
        end(request);
    }
    Ok(())
}

/// Badge 0: a standing rule the shell holds stands in for the ONCE, while its
/// node lives, and only for the calls its scope covers.
fn rules(
    shell: usize,
    ids: &mut impl Iterator<Item = u64>,
    stop: &dyn Fn(&str, u64, u64) -> Result<u64, Error>,
) -> Result<(), &'static str> {
    expect("no rule", stop("nobody", 0, 0), Err(Error::Permission))?;
    for (answer, other) in [
        (Answer::AlwaysTarget, Err(Error::Permission)),
        (Answer::AlwaysKind, Err(Error::NotFound)),
    ] {
        let id = ids.next().unwrap_or_default();
        let node = rule(shell, id, &process_stop("nobody"), answer)?;
        // Covered, so PROC_STOP's own answer: nothing has the name.
        expect("a rule's call", stop("nobody", 0, 0), Err(Error::NotFound))?;
        // Another target: only the rule for every target covers it.
        expect("a rule's kind", stop("nobody-else", 0, 0), other)?;
        if handle::rules::covers(shell, "service_stop(\"nobody\")") {
            return Err("proc-stop: a process_stop rule covered a service stop");
        }
        end(node);
        expect(
            "a removed rule",
            stop("nobody", 0, 0),
            Err(Error::Permission),
        )?;
    }
    Ok(())
}

/// An "Always allow" on menu `id`, which showed `call`, as the trusted path makes
/// it: the rule's node.
fn rule(shell: usize, id: u64, call: &str, answer: Answer) -> Result<GrantRef, &'static str> {
    let allowed = allow::allow(id, call, answer)
        .ok()
        .flatten()
        .ok_or("proc-stop: the trusted path could not make a rule")?;
    allow::end(&allowed);
    let handle = allowed
        .granted
        .rule
        .ok_or("proc-stop: an Always allow made no rule")?;
    handle::with(shell, |t| {
        t.table.get(kabi::handle::Handle(handle)).map(|e| e.grant)
    })
    .ok()
    .and_then(Result::ok)
    .flatten()
    .ok_or("proc-stop: the rule's handle names no node")
}

/// The supervisor's requests: refusals, then stop, start, undo and restart.
fn supervisor_side(shell: usize, ids: &mut impl Iterator<Item = u64>) -> Result<(), &'static str> {
    ask("stop spin", "err needs ONCE")?;
    ask("restart spin once=1", "err unknown request")?;
    ask("start spin once=1 id=2", "err a start takes only id=")?;
    // The kernel asks here, and holds no rule.
    ask("stop spin rule", "err rule NotFound")?;
    // A ONCE for a restart does not pass for a stop.
    let id = ids.next().unwrap_or_default();
    let (badge, request) = once(shell, id, &call(Request::Restart(SERVICE))?)?;
    ask(
        &Request::Stop(SERVICE).encode(Proof::Once(Once { badge, id })),
        "err ONCE Invalid",
    )?;
    end(request);
    // A start is run-tier: it asks nothing.
    ask("start shell", "err protected")?;
    ask("start spin", "err not stopped")?;
    for (asked, want) in [
        (Request::Stop("llm"), "err protected"),
        (Request::Stop("nobody"), "err unknown service"),
        (Request::Stop(SERVICE), "ok"),
    ] {
        allowed(shell, ids, asked, want)?;
    }
    stays_down()?;
    allowed(shell, ids, Request::Stop(SERVICE), "err already stopped")?;
    // Started as the shell's log entry 50: only that entry's undo stops it.
    ask("start spin id=50", "ok")?;
    up()?;
    ask("stop spin undo", "err unknown request")?;
    ask("stop spin undo id=49", "err not that entry's start")?;
    ask("stop spin undo id=50", "ok")?;
    stays_down()?;
    ask("start spin", "ok")?;
    up()?;
    // A start with no entry, or an undo already spent, undoes nothing.
    ask("stop spin undo id=50", "err not that entry's start")?;
    let before = process::named(SERVICE);
    allowed(shell, ids, Request::Restart(SERVICE), "ok")?;
    wait_within(START_WAIT_MS, || {
        let now = process::named(SERVICE);
        !now.is_empty() && now != before
    })
    .map_err(|_| "proc-stop: restart did not bring spin back as a new process")?;
    Ok(())
}

/// `spin` ends, and stays down past its restart backoff.
fn stays_down() -> Result<(), &'static str> {
    wait_within(STOP_WAIT_MS, || !running(SERVICE))
        .map_err(|_| "proc-stop: spin did not end on stop")?;
    threads::sleep_ms(STAYS_DOWN_MS);
    if running(SERVICE) || running(UNSUPERVISED) {
        return Err("proc-stop: a stopped process came back");
    }
    Ok(())
}

fn up() -> Result<(), &'static str> {
    wait_within(START_WAIT_MS, || running(SERVICE))
        .map_err(|_| "proc-stop: spin did not start again")
}

/// `asked` with a fresh ONCE for its call; a second try with the same ONCE is
/// refused, as it was used up.
fn allowed(
    shell: usize,
    ids: &mut impl Iterator<Item = u64>,
    asked: Request,
    want: &str,
) -> Result<(), &'static str> {
    let id = ids.next().unwrap_or_default();
    let (badge, request) = once(shell, id, &call(asked)?)?;
    let text = asked.encode(Proof::Once(Once { badge, id }));
    ask(&text, want)?;
    ask(&text, "err ONCE NotFound")?;
    end(request);
    Ok(())
}

/// A `ONCE` in the shell's table for menu `id`, which showed `call`, as the
/// trusted path's Allow mints it: its badge and request node.
fn once(shell: usize, id: u64, call: &str) -> Result<(u64, GrantRef), &'static str> {
    let request = grants::new_request(id).map_err(|_| "proc-stop: no request node")?;
    let digest = termcore::once::digest_call(id, call);
    let badge = grants::mint_once(shell, Kind::Grant, Rights::empty(), 0, request, digest)
        .and_then(|h| {
            handle::with(shell, |t| t.table.get(h).map(|e| e.badge))?.map_err(Error::from)
        })
        .map_err(|_| "proc-stop: cannot mint a ONCE")?;
    Ok((badge, request))
}

fn end(request: GrantRef) {
    let _ = grants::revoke(request);
}

fn process_stop(name: &str) -> String {
    format!("process_stop(\"{name}\")")
}

fn call(request: Request) -> Result<String, &'static str> {
    request.call().ok_or("proc-stop: a request with no call")
}

fn expect(
    what: &str,
    got: Result<u64, Error>,
    want: Result<u64, Error>,
) -> Result<(), &'static str> {
    if got != want {
        log!("proc-stop: PROC_STOP_ONCE ({what}) gave {got:?}, wanted {want:?}");
        return Err("proc-stop: PROC_STOP_ONCE gave the wrong answer");
    }
    Ok(())
}

fn running(name: &str) -> bool {
    !process::named(name).is_empty()
}

fn ask(request: &str, want: &str) -> Result<(), &'static str> {
    let reply = ipc::call("supervisor", request.as_bytes(), 5_000)
        .map_err(|_| "proc-stop: the supervisor did not answer")?;
    if reply != want.as_bytes() {
        log!(
            "proc-stop: {request:?} -> {:?}, wanted {want:?}",
            String::from_utf8_lossy(&reply)
        );
        return Err("proc-stop: the supervisor gave the wrong reply");
    }
    Ok(())
}

fn wait_within(ms: u64, mut done: impl FnMut() -> bool) -> Result<(), ()> {
    for _ in 0..ms / 10 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    Err(())
}
