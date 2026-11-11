//! The manifest's `net` line (ADR 0013 section 6, P5's per-app `NetDest`):
//! `none`, or the destination the agent's network handle may at most reach,
//! in `netdest`'s text form (`proto:tcp,ports:443,hosts:api.weather.example`),
//! and after a destination, optionally, the bytes a day that network may move
//! (`bytes/day=10m`, whole MiB: [`NetBudget`], 10 when the line does not say, at
//! most 512).
//!
//! It is a ceiling, never a grant: each host the agent reaches still asks the
//! person once (P5 task 13's per-app grant), and a fetch still runs only from a
//! host consent grounds. So the line is held to what an agent's network use
//! needs and no more:
//!
//! - TCP only, to named ports: an agent fetches over TLS, and `ports:any`,
//!   none, or a range of [`MAX_RANGE`] ports or more names nothing it can use.
//! - Host names, never literal addresses: what an agent reaches is what a
//!   person can read in the install ask, and connect follows resolve.
//! - At least one host, each a name of two labels or more, so no suffix is a
//!   whole top-level domain (`com`).

use crate::Refusal;
use crate::budgets::NetBudget;
use alloc::string::ToString;
use netdest::{AddrRange, Dest, PortRange, Protocol};

/// The widest port range that still names ports (C on #695: `1-65535` is
/// `any` written out).
pub const MAX_RANGE: u16 = 16;

/// The `net` line's value: `None` for `none`, or the destination, with the bytes a
/// day it may move (the default when the line does not say, none for `none`).
pub fn parse(value: &str) -> Result<(Option<Dest>, NetBudget), Refusal> {
    let (dest, word) = match value.split_once(' ') {
        Some((dest, word)) => (dest, Some(word)),
        None => (value, None),
    };
    let refuse = |why: &'static str| Refusal::Net(value.to_string(), why);
    if dest == "none" {
        return match word {
            None => Ok((None, NetBudget::NONE)),
            Some(_) => Err(refuse("a budget needs a destination")),
        };
    }
    let dest = Dest::parse(dest).map_err(refuse)?;
    if dest.protocol != Protocol::Tcp {
        return Err(refuse("an agent's network is tcp"));
    }
    if dest.addrs != AddrRange::NoAddrs {
        return Err(refuse("an agent names hosts, never addresses"));
    }
    let wide = matches!(dest.ports, PortRange::Range(low, high) if high - low >= MAX_RANGE);
    if wide || matches!(dest.ports, PortRange::Any | PortRange::NoPorts) {
        return Err(refuse("an agent names the ports it uses"));
    }
    if dest.any_host {
        return Err(refuse(
            "an agent names its hosts; hosts:any is the shell's alone",
        ));
    }
    if dest.hosts.is_empty() {
        return Err(refuse("an agent's network names at least one host"));
    }
    if dest.hosts.iter().any(|host| !host.contains('.')) {
        return Err(refuse("a host is a name of two labels or more"));
    }
    // The destination is judged first, so a bad destination is refused as it always was
    // whatever follows it, and the budget word second.
    let budget = match word {
        None => NetBudget::DEFAULT,
        Some(word) => NetBudget::parse(word)?,
    };
    Ok((Some(dest), budget))
}

/// The line's value as it is written: the budget follows a destination.
pub fn format(net: &Option<Dest>, budget: NetBudget) -> alloc::string::String {
    match net {
        None => "none".to_string(),
        Some(dest) => alloc::format!("{dest} {}", budget.format()),
    }
}
