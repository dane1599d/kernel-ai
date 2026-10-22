//! The terminal's network commands: `:net`'s address lines, `:resolve` and `:ping`.

use crate::log;
use crate::net::ping::{self, Outcome};
use crate::net::resolve::{self, DEFAULT_TIMEOUT_MS};
use crate::net::setup::Source;
use crate::{hpet, net};
use alloc::string::String;
use alloc::vec::Vec;
use netcore::{duration, ping::millis};

const DEFAULT_PINGS: u16 = 4;

/// The address, where it came from, the lease and the DNS servers.
pub fn status() {
    let Some(status) = net::status() else {
        log!("network: no card");
        return;
    };
    let Some((ip, prefix)) = status.address else {
        log!("network: no address yet; asking DHCP");
        return;
    };
    let via = status.gateway.map_or_else(
        || String::from(", no gateway"),
        |gw| alloc::format!(" via {gw}"),
    );
    match status.source {
        Source::Dhcp {
            server,
            lease_s,
            since_ns,
        } => {
            log!("network: {ip}/{prefix}{via}, from the DHCP server at {server}");
            let age = duration((hpet::nanos().saturating_sub(since_ns)) / 1_000_000_000);
            match lease_s {
                Some(lease) => log!(
                    "lease: {}, granted {age} ago; the client renews it at half time",
                    duration(lease.into())
                ),
                None => log!("lease: endless, granted {age} ago"),
            }
        }
        Source::Fallback => {
            log!("network: {ip}/{prefix}{via}, the fallback: DHCP was silent and is still asked")
        }
        Source::Static => log!("network: {ip}/{prefix}{via}, static (ip= on the command line)"),
        Source::Waiting => log!("network: {ip}/{prefix}{via}"),
    }
    let (servers, forced) = status.dns_servers();
    let servers: Vec<String> = servers
        .map(|(ip, port)| match port {
            netcore::config::DNS_PORT => alloc::format!("{ip}"),
            port => alloc::format!("{ip}:{port}"),
        })
        .collect();
    let (queries, hits) = resolve::counts();
    let from = if forced {
        "dns= on the command line"
    } else {
        "the network"
    };
    match servers.is_empty() {
        true => log!("dns: no server known (boot with dns=IP)"),
        false => log!(
            "dns: {} (from {from}); {queries} queries sent, {hits} answered from the cache",
            servers.join(", ")
        ),
    }
}

/// `:resolve <host>`.
pub fn resolve(host: &str) {
    match resolve::resolve(host, DEFAULT_TIMEOUT_MS, || false) {
        Ok(found) => {
            let addresses: Vec<String> = found
                .addresses
                .iter()
                .map(|ip| alloc::format!("{ip}"))
                .collect();
            let ttl = match found.cached {
                true => alloc::format!("cached, {} s left", found.ttl_s),
                false => alloc::format!("TTL {} s", found.ttl_s),
            };
            log!("resolve: {host} is {} ({ttl})", addresses.join(", "));
        }
        Err(error) => log!("resolve: {host}: {error}"),
    }
}

/// `:ping <host|ip> [count]`.
pub fn ping(args: &str) {
    let mut words = args.split_whitespace();
    let host = words.next().unwrap_or("");
    let count = match words.next().map(str::parse::<u16>) {
        None => DEFAULT_PINGS,
        Some(Ok(n)) if (1..=ping::MAX_COUNT).contains(&n) => n,
        Some(_) => {
            log!(
                "usage: :ping <host|ip> [count], count 1 to {}",
                ping::MAX_COUNT
            );
            return;
        }
    };
    let target = match resolve::resolve(host, DEFAULT_TIMEOUT_MS, || false) {
        Ok(found) => found.addresses[0],
        Err(error) => {
            log!("ping: {host}: {error}");
            return;
        }
    };
    log!("ping: {host} ({target}), {count} echo requests");
    let each = |seq: u16, outcome: Outcome| match outcome {
        Outcome::Reply(us) => log!("ping: reply from {target}, seq {seq}, {} ms", millis(us)),
        Outcome::Lost => log!("ping: seq {seq}, no reply within {} ms", ping::INTERVAL_MS),
    };
    match ping::ping(target, count, each, || false) {
        Ok(stats) => log!("ping: {host}: {}", stats.summary()),
        Err(error) => log!("ping: {host}: {error}"),
    }
}
