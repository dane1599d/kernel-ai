//! `test=https-mock` (P5 task 12's harness): xtask's HTTPS mock reached from
//! ring 3, as every P5 case that fetches will reach it. `httpscheck`, with NET
//! and a `NetDest` for three of the mock's hosts, resolves them through the
//! `netcore::fixture` zone (`dns=`), reaches the mock through QEMU's forward
//! of the zone's mock address on port 443, and trusts the mock's test CA from
//! the boot bundle's `netroots/1.der`:
//! - a GET from the repository host is a 200;
//! - a host the handle does not name is refused before anything is sent;
//! - a certificate for another name, and one from a CA the client does not
//!   trust, are refused in the handshake.
//!
//! `test=fetch-service` (P5 task 12): the `fetch` service, started with SERVE
//! and NET and keeping no handle of its own, and `httpscheck` as its caller
//! (`via=fetch`), with a route to it and a `NetDest` for three of the mock's
//! hosts, narrowing one exact host a call as comphost and the shell will.

use crate::fpu_tests::wait_exit;
use crate::process::{self, SpawnRequest};
use crate::{log, net, threads};
use kabi::{ExitReason, Limits, cap};
use netcore::fixture;

/// How long the network may take to come up.
const SETUP_WAIT_MS: u64 = 15_000;
/// A TLS client's room: rustls, the roots, a 64 KiB body.
const LIMITS: Limits = Limits {
    max_pages: 4096,
    max_burst_ms: 60_000,
    caps: cap::NET,
    stack_pages: 64,
};

/// The handle each run holds: three of the mock's hosts, on 443. The mock's
/// other host, [`fixture::MOCK_OFF`], is left out.
fn spec() -> alloc::string::String {
    alloc::format!(
        "proto:tcp,ports:443,hosts:{}+{}+{}",
        fixture::MOCK_REPO,
        fixture::MOCK_RELEASES,
        fixture::MOCK_ASSETS
    )
}

/// A new record for [`spec`], and a `NetDest` handle's entry for it.
fn netdest() -> Result<captable::Entry, &'static str> {
    let dest = netdest::Dest::parse(&spec()).map_err(|_| "https-mock: a bad destination")?;
    let live = |object| crate::handle::names(kabi::handle::Kind::NetDest, object);
    let object = net::dest::create(dest, live).ok_or("https-mock: no room for a NetDest")?;
    Ok(captable::Entry::new(
        kabi::handle::Kind::NetDest,
        crate::handle::NETDEST_RIGHTS,
        object,
    ))
}

/// Runs `httpscheck` as `name` with `args` and a fresh handle; whether it got
/// what it was told to expect.
fn httpscheck(name: &str, args: &str) -> Result<(), &'static str> {
    let request = SpawnRequest {
        name,
        program: "httpscheck",
        args: args.as_bytes(),
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    let pid = process::spawn_with(&request, alloc::vec![netdest()?], false)
        .map_err(|_| "https-mock: cannot start httpscheck")?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => Ok(()),
        _ => Err("https-mock: httpscheck did not get what it expected"),
    }
}

/// Waits for the network's address.
fn wait_up() -> Result<(), &'static str> {
    for _ in 0..SETUP_WAIT_MS / 10 {
        if net::status().is_some_and(|s| s.address.is_some()) {
            return Ok(());
        }
        threads::sleep_ms(10);
    }
    Err("https: the network was not set up in time")
}

/// The `fetch` service's room: a TLS client, and a 64 KiB body at a time.
const FETCH_LIMITS: Limits = Limits {
    caps: cap::SERVE | cap::NET,
    ..LIMITS
};

/// Its caller's: it narrows handles and calls, and needs no NET.
const CALLER_LIMITS: Limits = Limits {
    caps: cap::CALL,
    ..LIMITS
};

pub fn fetch_check() -> Result<(), &'static str> {
    wait_up()?;
    let service = SpawnRequest {
        name: "fetch",
        program: "fetch",
        args: b"",
        limits: FETCH_LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    };
    process::spawn_with(&service, alloc::vec![], false)
        .map_err(|_| "fetch-service: cannot start fetch")?;
    let caller = SpawnRequest {
        name: "fetch-caller",
        program: "httpscheck",
        args: b"via=fetch",
        limits: CALLER_LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &["fetch"],
    };
    let pid = process::spawn_with(&caller, alloc::vec![netdest()?], false)
        .map_err(|_| "fetch-service: cannot start its caller")?;
    match wait_exit(pid)? {
        (ExitReason::Exited, 0) => {}
        _ => return Err("fetch-service: the caller did not get what it expected"),
    }
    log!(
        "fetch-service: fetch answered on each caller's handle: a body, a redirect within the host followed, one off it handed back and fetched on its own handle, and a host off the handle refused"
    );
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    wait_up()?;
    let url = |host: &str| alloc::format!("https://{host}/hello");
    httpscheck(
        "https-get",
        &alloc::format!("get={}", url(fixture::MOCK_REPO)),
    )?;
    httpscheck(
        "https-refused",
        &alloc::format!("refused={}", url(fixture::MOCK_OFF)),
    )?;
    httpscheck(
        "https-name",
        &alloc::format!("tls={}", url(fixture::MOCK_RELEASES)),
    )?;
    httpscheck(
        "https-ca",
        &alloc::format!("tls={}", url(fixture::MOCK_ASSETS)),
    )?;
    log!(
        "https-mock: the mock's hosts resolve and answer over TLS with its test CA; another host is refused, and a wrong name and an unknown CA are refused in the handshake"
    );
    Ok(())
}
