//! The manifest's `net` line (ADR 0013 section 6, P5's per-app `NetDest`):
//! `none`, or the destination the agent's network handle may at most reach,
//! in `netdest`'s text form (`proto:tcp,ports:443,hosts:api.weather.example`).
//!
//! It is a ceiling, never a grant: each host the agent reaches still asks the
//! person once (P5 task 13's per-app grant), and a fetch still runs only from a
//! host consent grounds. So the line is held to what an agent's network use
//! needs and no more:
//!
//! - TCP only, to named ports: an agent fetches over TLS, and `ports:any` or
//!   none names nothing it can use.
//! - Host names, never literal addresses: what an agent reaches is what a
//!   person can read in the install ask, and connect follows resolve.
//! - At least one host, each a name of two labels or more, so no suffix is a
//!   whole top-level domain (`com`).

use crate::Refusal;
use alloc::string::ToString;
use netdest::{AddrRange, Dest, PortRange, Protocol};

/// The `net` line's value: `None` for `none`, or the destination.
pub fn parse(value: &str) -> Result<Option<Dest>, Refusal> {
    if value == "none" {
        return Ok(None);
    }
    let refuse = |why: &'static str| Refusal::Net(value.to_string(), why);
    let dest = Dest::parse(value).map_err(refuse)?;
    if dest.protocol != Protocol::Tcp {
        return Err(refuse("an agent's network is tcp"));
    }
    if dest.addrs != AddrRange::NoAddrs {
        return Err(refuse("an agent names hosts, never addresses"));
    }
    if matches!(dest.ports, PortRange::Any | PortRange::NoPorts) {
        return Err(refuse("an agent names the ports it uses"));
    }
    if dest.hosts.is_empty() {
        return Err(refuse("an agent's network names at least one host"));
    }
    if dest.hosts.iter().any(|host| !host.contains('.')) {
        return Err(refuse("a host is a name of two labels or more"));
    }
    Ok(Some(dest))
}

/// The line's value as it is written.
pub fn format(net: &Option<Dest>) -> alloc::string::String {
    match net {
        None => "none".to_string(),
        Some(dest) => dest.to_string(),
    }
}
