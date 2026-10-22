//! P5's network setup cases, on QEMU's user network (slirp: DHCP server and
//! gateway at 10.0.2.2, DNS proxy at 10.0.2.3) with xtask's DNS responder for the
//! `netcore::fixture` zone named by `dns=`, so nothing needs the internet.
//!
//! - `test=net-setup`: DHCP gives 10.0.2.15/24 via 10.0.2.2 with DNS 10.0.2.3; the
//!   gateway answers pings; names resolve, CNAMEs too, from the cache within their
//!   TTL and from the server after it; NXDOMAIN and silence fail the right way;
//!   ring 3 resolves through RESOLVE with the RESOLVE capability and is refused
//!   without it; and the lease, capped at 4 s, is renewed rather than lost.
//! - `test=net-fallback`: with DHCP sent to a port nobody serves, the fallback
//!   address comes after `dhcp_wait_ms` and the gateway answers on it.
//! - `test=net-static`: `ip=static` is used at once, with no DHCP.

use crate::fpu_tests::wait_exit;
use crate::net::ping::{self, Outcome};
use crate::net::resolve::{self, Resolved};
use crate::net::setup::{Source, Status};
use crate::process::{self, SpawnRequest};
use crate::{hpet, log, net, net_commands, threads};
use core::net::Ipv4Addr;
use kabi::{ExitReason, Limits, cap};
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
    caps: cap::RESOLVE,
    stack_pages: 0,
};

/// `netcheck` resolves kai.test through RESOLVE with its own capability, and is
/// refused without it, NET alone included: NET is for HTTP to the one server.
fn ring3_checks() -> Result<(), &'static str> {
    let runs = [
        ("netcheck", cap::RESOLVE, "host=kai.test expect=10.0.2.99"),
        ("netcheck-nocap", 0, "host=kai.test expect=permission"),
        ("netcheck-net", cap::NET, "host=kai.test expect=permission"),
    ];
    for (name, caps, args) in runs {
        let pid = process::spawn(&SpawnRequest {
            name,
            program: "netcheck",
            args: args.as_bytes(),
            limits: Limits {
                caps,
                ..NETCHECK_LIMITS
            },
            spawner: 0,
            supervisor: false,
        })
        .map_err(|_| "net-setup: cannot start netcheck")?;
        if !matches!(wait_exit(pid)?, (ExitReason::Exited, 0)) {
            return Err("net-setup: netcheck did not get what it expected");
        }
    }
    log!("net-setup: ring 3 resolved kai.test with RESOLVE and was refused with NET alone or none");
    Ok(())
}
