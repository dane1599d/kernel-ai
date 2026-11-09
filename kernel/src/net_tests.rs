//! P5's network setup cases, on QEMU's user network (slirp: DHCP server and
//! gateway at 10.0.2.2, DNS proxy at 10.0.2.3) with xtask's DNS responder for the
//! `netcore::fixture` zone named by `dns=`, so nothing needs the internet.
//!
//! - `test=net-setup`: DHCP gives 10.0.2.15/24 via 10.0.2.2 with DNS 10.0.2.3; the
//!   gateway answers pings; names resolve, CNAMEs too, from the cache within their
//!   TTL and from the server after it; NXDOMAIN and silence fail the right way;
//!   ring 3 resolves through RESOLVE_ON on a `NetDest` handle and is refused
//!   without one; and the lease, capped at 4 s, is renewed rather than lost.
//! - `test=net-fallback`: with DHCP sent to a port nobody serves, the fallback
//!   address comes after `dhcp_wait_ms` and the gateway answers on it.
//! - `test=net-static`: `ip=static` is used at once, with no DHCP.
//! - `test=net-dest` (P5 task 1): what a `NetDest` handle allows ([`dest_check`]).
//! - `test=net-stream` (P5 task 2): TCP streams on one ([`stream_check`]).

use crate::fpu_tests::wait_exit;
use crate::net::ping::{self, Outcome};
use crate::net::resolve::{self, Resolved};
use crate::net::setup::{Source, Status};
use crate::process::{self, SpawnRequest};
use crate::{hpet, log, net, net_commands, threads};
use core::net::Ipv4Addr;
use kabi::{ExitReason, Limits};
use netcore::config::{FALLBACK, FALLBACK_DNS};
use netcore::fixture;

const SETUP_WAIT_MS: u64 = 15_000;
const GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
const PINGS: u16 = 3;
/// The silent name's own time limit, and how much later than that failing is fine.
const SILENT_TIMEOUT_MS: u64 = 1_500;
const SILENT_SLACK_MS: u64 = 1_500;
/// The lease `dhcp_max_lease_s=` caps, and how long the case watches it after.
const CAPPED_LEASE_S: u32 = 4;
const LEASE_WATCH_MS: u64 = 10_000;

/// `test=net-setup`.
pub fn setup_check() -> Result<(), &'static str> {
    let status = wait_for(|s| matches!(s.source, Source::Dhcp { .. }))?;
    let Source::Dhcp {
        lease_s, since_ns, ..
    } = status.source
    else {
        return Err("net-setup: no DHCP lease");
    };
    check_nat_network(&status, "DHCP")?;
    if lease_s != Some(CAPPED_LEASE_S) {
        return Err("net-setup: the lease was not capped at 4 s by dhcp_max_lease_s=");
    }
    log!("net-setup: DHCP gave 10.0.2.15/24 via 10.0.2.2, DNS 10.0.2.3, a 4 s lease");
    ping_gateway("net-setup")?;
    resolve_checks()?;
    ring3_checks()?;
    // The lease would have run out by now three times over without renewals.
    let watch_until = since_ns + LEASE_WATCH_MS * 1_000_000;
    threads::sleep_ms(watch_until.saturating_sub(hpet::nanos()) / 1_000_000);
    let status = net::status().ok_or("net-setup: no card")?;
    if status.losses != 0 || !matches!(status.source, Source::Dhcp { .. }) {
        return Err("net-setup: the capped lease ran out instead of being renewed");
    }
    log!("net-setup: the 4 s lease was renewed: still leased after 10 s, none lost");
    // What `:net`, `:resolve` and `:ping` print; xtask checks the text.
    net_commands::status();
    net_commands::resolve(fixture::ALIAS);
    net_commands::ping("10.0.2.2 1");
    Ok(())
}

/// `test=net-fallback`.
pub fn fallback_check() -> Result<(), &'static str> {
    let status = wait_for(|s| s.source == Source::Fallback)?;
    check_nat_network(&status, "the fallback")?;
    if status.leases != 0 {
        return Err("net-fallback: a lease came though nobody serves DHCP on that port");
    }
    log!("net-fallback: DHCP was silent; the fallback 10.0.2.15/24 via 10.0.2.2 is up");
    ping_gateway("net-fallback")?;
    net_commands::status();
    Ok(())
}

/// `test=net-static`.
pub fn static_check() -> Result<(), &'static str> {
    let status = net::status().ok_or("net-static: no card")?;
    if status.source != Source::Static {
        return Err("net-static: ip=static was not applied at boot");
    }
    check_nat_network(&status, "ip=static")?;
    log!("net-static: ip=static gave 10.0.2.15/24 via 10.0.2.2 at boot, without DHCP");
    ping_gateway("net-static")?;
    let found = resolve::resolve(fixture::KAI, resolve::DEFAULT_TIMEOUT_MS, || false)?;
    if found.addresses != [Ipv4Addr::from(fixture::KAI_ADDRESS)] {
        return Err("net-static: kai.test did not resolve to 10.0.2.99");
    }
    log!("net-static: kai.test resolved to 10.0.2.99 through dns=");
    Ok(())
}

fn wait_for(ready: impl Fn(&Status) -> bool) -> Result<Status, &'static str> {
    for _ in 0..SETUP_WAIT_MS / 10 {
        if let Some(status) = net::status().filter(&ready) {
            return Ok(status);
        }
        threads::sleep_ms(10);
    }
    log!("net: setup never got there: {:?}", net::status());
    Err("net: the network was not set up in time")
}

/// The NAT network's address, gateway and DNS server.
fn check_nat_network(status: &Status, how: &str) -> Result<(), &'static str> {
    let named_dns = status.named_dns();
    if status.address != Some((FALLBACK.address, FALLBACK.prefix))
        || status.gateway != FALLBACK.gateway
        || named_dns[0] != Some(FALLBACK_DNS)
    {
        log!("net: {how} gave {status:?}");
        return Err("net: not 10.0.2.15/24 via 10.0.2.2 with DNS 10.0.2.3");
    }
    Ok(())
}

fn ping_gateway(case: &str) -> Result<(), &'static str> {
    let mut answered = 0;
    let stats = ping::ping(
        GATEWAY,
        PINGS,
        |_, outcome| answered += u16::from(matches!(outcome, Outcome::Reply(_))),
        || false,
    )?;
    if answered != PINGS {
        log!("{case}: ping 10.0.2.2: {}", stats.summary());
        return Err("net: the gateway did not answer every ping");
    }
    log!("{case}: {PINGS} of {PINGS} pings to the gateway answered");
    Ok(())
}

fn lookup(host: &str) -> Result<Resolved, &'static str> {
    resolve::resolve(host, resolve::DEFAULT_TIMEOUT_MS, || false)
}

fn resolve_checks() -> Result<(), &'static str> {
    let kai = Ipv4Addr::from(fixture::KAI_ADDRESS);
    let (queries, _) = resolve::counts();
    let first = lookup(fixture::KAI)?;
    if first.addresses != [kai] || first.cached || first.ttl_s != u64::from(fixture::KAI_TTL) {
        return Err("net-setup: kai.test did not resolve to 10.0.2.99 with TTL 300");
    }
    let again = lookup(fixture::KAI)?;
    let (after, hits) = resolve::counts();
    if again.addresses != [kai] || !again.cached || after != queries + 1 || hits == 0 {
        return Err("net-setup: the second kai.test was not answered from the cache");
    }
    log!("net-setup: kai.test resolved to 10.0.2.99 (TTL 300 s), then from the cache");
    let alias = lookup(fixture::ALIAS)?;
    if alias.addresses != [kai] || alias.ttl_s != u64::from(fixture::ALIAS_TTL) {
        return Err("net-setup: alias.kai.test did not follow its CNAME to 10.0.2.99");
    }
    log!("net-setup: alias.kai.test followed its CNAME to 10.0.2.99 (TTL 60 s)");
    let short = Ipv4Addr::from(fixture::SHORT_ADDRESS);
    let first = lookup(fixture::SHORT)?;
    threads::sleep_ms(u64::from(fixture::SHORT_TTL) * 1000 + 500);
    let (before, _) = resolve::counts();
    let later = lookup(fixture::SHORT)?;
    let (after, _) = resolve::counts();
    if first.addresses != [short]
        || later.addresses != [short]
        || later.cached
        || after != before + 1
    {
        return Err("net-setup: short.kai.test was still cached after its 1 s TTL");
    }
    log!("net-setup: short.kai.test was asked again once its 1 s TTL ran out");
    match lookup(fixture::MISSING) {
        Err("dns: no such host") => log!("net-setup: missing.kai.test: no such host"),
        _ => return Err("net-setup: missing.kai.test did not fail with no such host"),
    }
    let started = hpet::nanos();
    let silent = resolve::resolve(fixture::SILENT, SILENT_TIMEOUT_MS, || false);
    let took_ms = (hpet::nanos() - started) / 1_000_000;
    match silent {
        Err("dns: timed out") if took_ms <= SILENT_TIMEOUT_MS + SILENT_SLACK_MS => {
            log!("net-setup: silent.kai.test timed out after {SILENT_TIMEOUT_MS} ms as asked")
        }
        _ => return Err("net-setup: silent.kai.test did not time out in time"),
    }
    match lookup("bad..name") {
        Err("dns: host name has a bad label") => Ok(()),
        _ => Err("net-setup: a malformed name was not refused"),
    }
}

const NETCHECK_LIMITS: Limits = Limits {
    max_pages: 256,
    max_burst_ms: 10_000,
    caps: 0,
    stack_pages: 0,
};

/// What the `netcheck` runs' handle allows: names under kai.test, and
/// 10.0.2.2:8000 by address.
const KAI_TEST: &str = "proto:tcp,addrs:10.0.2.2,ports:8000,hosts:kai.test";

/// A new record for `spec`, and a `NetDest` handle's entry for it.
fn netdest(spec: &str) -> Result<(u64, captable::Entry), &'static str> {
    let dest = netdest::Dest::parse(spec).map_err(|_| "net: a bad test destination")?;
    let live = |object| crate::handle::names(kabi::handle::Kind::NetDest, object);
    let object = net::dest::create(dest, live).ok_or("net: no room for a NetDest record")?;
    let entry = captable::Entry::new(
        kabi::handle::Kind::NetDest,
        crate::handle::NETDEST_RIGHTS,
        object,
    );
    Ok((object, entry))
}

/// Runs `netcheck` as `name` with `args`, holding `dest` if given; whether it
/// got what it expected.
fn netcheck(name: &str, args: &str, dest: Option<captable::Entry>) -> Result<(), &'static str> {
    let request = SpawnRequest {
        name,
        program: "netcheck",
        args: args.as_bytes(),
        limits: NETCHECK_LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    let pid = process::spawn_with(&request, dest.into_iter().collect(), false)
        .map_err(|_| "net: cannot start netcheck")?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => Ok(()),
        _ => Err("net: netcheck did not get what it expected"),
    }
}

/// `netcheck` resolves kai.test on a `NetDest` handle that names it, and is
/// refused without one, NET alone included: NET is for HTTP to the one server.
fn ring3_checks() -> Result<(), &'static str> {
    let (_, entry) = netdest(KAI_TEST)?;
    netcheck("netcheck", "host=kai.test expect=10.0.2.99", Some(entry))?;
    netcheck("netcheck-nocap", "host=kai.test expect=permission", None)?;
    log!("net-setup: ring 3 resolved kai.test on its NetDest handle and was refused with none");
    Ok(())
}

/// `test=net-dest` (P5 task 1): what a `NetDest` handle allows.
/// - A name under its suffixes resolves, and its addresses then count for a
///   connect on their ports while their TTL lasts; other addresses and ports
///   do not, but the literal range does.
/// - A name under none is refused before any query leaves.
/// - A narrower handle may be made, and allows only what it names; a wider
///   one is refused.
/// - The retired RESOLVE call refuses everyone.
pub fn dest_check() -> Result<(), &'static str> {
    use netdest::Protocol::Tcp;
    wait_for(|s| s.address.is_some())?;
    let (object, entry) = netdest(KAI_TEST)?;
    netcheck(
        "netdest-a",
        "host=alias.kai.test expect=10.0.2.99",
        Some(entry),
    )?;
    let resolved = Ipv4Addr::new(10, 0, 2, 99);
    let connects = [
        (resolved, 8000, true),
        (resolved, 22, false),
        (GATEWAY, 8000, true),
        (Ipv4Addr::new(10, 0, 2, 3), 8000, false),
    ];
    for (addr, port, want) in connects {
        if net::dest::may_connect(object, Tcp, addr, port) != want {
            log!("net-dest: a connect to {addr}:{port} was not {want}");
            return Err("net-dest: a connect check went the wrong way");
        }
    }
    log!(
        "net-dest: alias.kai.test resolved on the handle, and only its address and the literal range may be connected to, on port 8000"
    );
    let (queries, _) = resolve::counts();
    let (_, entry) = netdest(KAI_TEST)?;
    netcheck(
        "netdest-b",
        "host=leak.attacker.example expect=permission",
        Some(entry),
    )?;
    if resolve::counts().0 != queries {
        return Err("net-dest: a refused name was still looked up");
    }
    log!("net-dest: a name under none of its hosts was refused, and no query left");
    let (_, entry) = netdest(KAI_TEST)?;
    let narrow = "narrow=proto:tcp,hosts:alias.kai.test";
    netcheck(
        "netdest-c",
        &alloc::format!("{narrow} host=alias.kai.test expect=10.0.2.99"),
        Some(entry),
    )?;
    let (_, entry) = netdest(KAI_TEST)?;
    netcheck(
        "netdest-d",
        &alloc::format!("{narrow} host=kai.test expect=permission"),
        Some(entry),
    )?;
    let (_, entry) = netdest(KAI_TEST)?;
    let wider = "narrow=proto:tcp,ports:any,hosts:kai.test host=kai.test expect=narrow-refused";
    netcheck("netdest-e", wider, Some(entry))?;
    log!("net-dest: a narrower handle allowed only its own host, and a wider one was refused");
    let (_, entry) = netdest(KAI_TEST)?;
    netcheck(
        "netdest-f",
        "legacy host=kai.test expect=permission",
        Some(entry),
    )?;
    log!("net-dest: the retired RESOLVE refused even a holder of a NetDest");
    any_host_pinned()
}

/// The shell's ceiling, `hosts:any` (#760), as its plan line would make it.
const ANY_HOST: &str = "proto:tcp,ports:443,hosts:any";

/// A `hosts:any` record stays with the process it was made for: ring 3 cannot
/// narrow another out of it, or a suffix, though it narrows to one exact host,
/// and a handle to it never moves, though its exact narrowing does.
fn any_host_pinned() -> Result<(), &'static str> {
    let (object, entry) = netdest(ANY_HOST)?;
    if !net::dest::any_host(object) {
        return Err("net-dest: a hosts:any record was not marked");
    }
    netcheck("netdest-h", "any=pinned", Some(entry))?;
    // The move, from the kernel side in a spare slot's table, as
    // `handle_tests` does: the receiver may hold a NetDest, so only the pin
    // can refuse it.
    let (object, ceiling) = netdest(ANY_HOST)?;
    let exact = netdest::Dest::parse("proto:tcp,ports:443,hosts:kai.test,match:exact")
        .map_err(|_| "net: a bad test destination")?;
    let live = |object| crate::handle::names(kabi::handle::Kind::NetDest, object);
    let narrowed = net::dest::narrow(object, exact, live)
        .map_err(|_| "net-dest: the ceiling did not narrow to one exact host")?;
    let one = captable::Entry::new(
        kabi::handle::Kind::NetDest,
        crate::handle::NETDEST_RIGHTS,
        narrowed,
    );
    let slot = crate::process::MAX_PROCESSES - 1;
    crate::handle::create(slot, kabi::cap::NET, 99, &[])
        .map_err(|_| "net-dest: cannot make a spare table")?;
    let given = crate::handle::give(slot, alloc::vec![ceiling, one]);
    let moved = |handle| crate::handle::take_for(slot, &[handle], kabi::cap::NET);
    let (whole, part) = match given.as_slice() {
        [whole, part] => (moved(*whole), moved(*part)),
        _ => (Ok(alloc::vec::Vec::new()), Err(kabi::Error::NoMemory)),
    };
    crate::handle::destroy(slot);
    match (whole, part) {
        (Err(kabi::Error::Permission), Ok(_)) => {
            log!(
                "net-dest: hosts:any stays where it was made: no narrowing makes another and it never moves, while its one-host narrowing does"
            );
            Ok(())
        }
        _ => Err("net-dest: a hosts:any handle moved, or its narrowing did not"),
    }
}

/// `test=net-stream` (P5 task 2): TCP streams on a `NetDest` handle, to xtask's
/// echo server (`echo=`), from `netcheck`'s stream modes.
/// - A stream and a bulk one echo 8 KiB and a line; a send over 8 KiB is
///   TooBig; a port the handle does not name is refused; the 17th stream of a
///   process is Busy; each closes, and none is left open.
/// - A process killed while it holds three streams leaves none open.
pub fn stream_check(cmdline: &str) -> Result<(), &'static str> {
    let (server, port) = cmdline
        .split_whitespace()
        .find_map(|w| net::parse_endpoint(w.strip_prefix("echo=")?))
        .ok_or("net-stream: no echo= on the command line")?;
    wait_for(|s| s.address.is_some())?;
    let spec = alloc::format!("proto:tcp,addrs:{server},ports:{port}");
    let (_, entry) = netdest(&spec)?;
    netcheck(
        "netstream-a",
        &alloc::format!("stream=echo port={port}"),
        Some(entry),
    )?;
    if net::stream::count(0).1 != 0 {
        return Err("net-stream: a closed stream was left open");
    }
    log!("net-stream: streams echoed through the handle, kept to its port and to 16, and closed");
    let (_, entry) = netdest(&spec)?;
    let args = alloc::format!("stream=hold port={port}");
    let request = SpawnRequest {
        name: "netstream-b",
        program: "netcheck",
        args: args.as_bytes(),
        limits: NETCHECK_LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    let pid = process::spawn_with(&request, alloc::vec![entry], false)
        .map_err(|_| "net-stream: cannot start netcheck")?;
    let mut held = false;
    for _ in 0..SETUP_WAIT_MS / 10 {
        if net::stream::count(pid).0 == 3 {
            held = true;
            break;
        }
        threads::sleep_ms(10);
    }
    if !held {
        return Err("net-stream: netcheck never held 3 streams");
    }
    process::kill(pid, ExitReason::Killed).map_err(|_| "net-stream: cannot kill netcheck")?;
    wait_exit(pid)?;
    if net::stream::count(pid) != (0, 0) {
        return Err("net-stream: a killed process's streams stayed open");
    }
    log!("net-stream: a process killed with 3 streams open left none");
    Ok(())
}
