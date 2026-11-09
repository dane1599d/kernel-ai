//! What a `NetDest` handle names (ADR 0001 D2, P5 task 1): the addresses and
//! remote ports it may connect to, its protocol, and the host-name suffixes it
//! may resolve. The kernel holds these; a process holds only a handle to one.
//!
//! - A resolve answers only a name under one of the destination's suffixes
//!   ([`Dest::allows_name`]).
//! - A connect reaches a literal address and port in its ranges
//!   ([`Dest::allows_literal`]), or an address this destination resolved
//!   within its TTL (the kernel keeps those).
//! - A destination only shrinks: a narrower one is made from a wider one only
//!   when the wider one covers it ([`Dest::within`]).
//!
//! The text form, as a plan line's `netdest=` gives it, is comma-separated
//! `field:value` pairs, values joined by `+`:
//! `proto:tcp,addrs:10.0.2.0/24,ports:80+443,hosts:kai.test`. Addresses are
//! one subnet or up to eight addresses, and ports one range or up to eight ports.
//! `any` and `none` stand for every or no address or port. A field left out is
//! `none` (hosts: no names), so a destination allows only what it names.
//! `match:exact` makes each host name only itself, not the names under it, as a
//! per-host grant needs (P5 task 13); it is left out for suffixes.
//! `hosts:any` names every host name of two labels or more, for one ceiling
//! only: the shell's plan line, on port 443 (the founder, 2026-11-09: Kai may
//! reach any website the user names). It is never exact, and only a handle
//! that has it can make another that has it; who may hold it is the plan
//! parser's rule (`supervise`), not this crate's.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod ranges;
#[cfg(test)]
mod tests;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::net::Ipv4Addr;
pub use ranges::{AddrRange, MAX_SET_SIZE, PortRange};

/// The most host-name suffixes a destination names.
pub const MAX_HOSTS: usize = 8;
/// The longest host name or suffix, as DNS allows.
pub const MAX_HOST: usize = 253;
/// The longest text form accepted.
pub const MAX_TEXT: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

/// One destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dest {
    pub protocol: Protocol,
    pub addrs: AddrRange,
    pub ports: PortRange,
    /// Lower-case host names; a name is under one if it is it, or ends with
    /// `.` and it.
    pub hosts: Vec<String>,
    /// The hosts name only themselves (`match:exact`), not the names under
    /// them: a per-app grant of one host (P5 task 13).
    pub exact: bool,
    /// Every host name of two labels or more (`hosts:any`), in place of
    /// [`Dest::hosts`], which is then empty. Never with [`Dest::exact`].
    pub any_host: bool,
}

impl Dest {
    /// Whether a resolve of `name` may be answered: its [`normalize`]d form
    /// is under one of the suffixes. The caller looks up that same form.
    pub fn allows_name(&self, name: &str) -> bool {
        normalize(name).is_some_and(|name| self.names(&name))
    }

    /// Whether the normalised `name` is one of the hosts, or (unless
    /// [`Dest::exact`]) under one.
    fn names(&self, name: &str) -> bool {
        if self.any_host {
            return name.contains('.');
        }
        self.hosts.iter().any(|host| match self.exact {
            true => name == host,
            false => under(name, host),
        })
    }

    /// Whether a connect to `addr`:`port` is in the literal ranges.
    pub fn allows_literal(&self, protocol: Protocol, addr: Ipv4Addr, port: u16) -> bool {
        self.protocol == protocol
            && self.addrs.is_addr_valid(addr)
            && self.ports.is_port_valid(port)
    }

    /// Whether a connect to `port` may go to an address this destination
    /// resolved: the ports and the protocol still bound it.
    pub fn allows_resolved_port(&self, protocol: Protocol, port: u16) -> bool {
        self.protocol == protocol && self.ports.is_port_valid(port)
    }

    /// Whether `self` allows nothing `outer` does not: a handle to `self` may
    /// be made from a handle to `outer`.
    pub fn within(&self, outer: &Dest) -> bool {
        self.protocol == outer.protocol
            && outer.addrs.covers(&self.addrs)
            && outer.ports.covers(&self.ports)
            // An exact outer covers only exact hosts it names; a suffix outer
            // covers any host or suffix under it.
            && (self.exact || !outer.exact)
            // Only `hosts:any` covers `hosts:any`.
            && match self.any_host {
                true => outer.any_host,
                false => self.hosts.iter().all(|host| outer.names(host)),
            }
    }

    /// Reads the text form (see the crate's docs).
    pub fn parse(text: &str) -> Result<Dest, &'static str> {
        if text.is_empty() || text.len() > MAX_TEXT {
            return Err("netdest: empty or too long");
        }
        let mut dest = Dest {
            protocol: Protocol::Tcp,
            addrs: AddrRange::NoAddrs,
            ports: PortRange::NoPorts,
            hosts: Vec::new(),
            exact: false,
            any_host: false,
        };
        let mut seen: Vec<&str> = Vec::new();
        for field in text.split(',') {
            let (key, value) = field.split_once(':').ok_or("netdest: a field has no ':'")?;
            if seen.contains(&key) {
                return Err("netdest: a field is given twice");
            }
            seen.push(key);
            match key {
                "proto" => dest.protocol = parse_protocol(value)?,
                "addrs" => dest.addrs = parse_addrs(value)?,
                "ports" => dest.ports = parse_ports(value)?,
                "hosts" if value == "any" => dest.any_host = true,
                "hosts" => dest.hosts = parse_hosts(value)?,
                "match" => {
                    dest.exact = match value {
                        "exact" => true,
                        "suffix" => false,
                        _ => return Err("netdest: match is exact or suffix"),
                    }
                }
                _ => return Err("netdest: unknown field (proto, addrs, ports, hosts, match)"),
            }
        }
        if dest.any_host && dest.exact {
            return Err("netdest: hosts:any names no host exactly");
        }
        Ok(dest)
    }
}

impl fmt::Display for Dest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let protocol = match self.protocol {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        };
        write!(f, "proto:{protocol},addrs:")?;
        match &self.addrs {
            AddrRange::Any => f.write_str("any")?,
            AddrRange::NoAddrs => f.write_str("none")?,
            AddrRange::Addr(a) => write!(f, "{a}")?,
            AddrRange::Subnet(a, p) => write!(f, "{a}/{p}")?,
            AddrRange::AddrSet(set) => join(f, set.iter())?,
        }
        f.write_str(",ports:")?;
        match &self.ports {
            PortRange::Any => f.write_str("any")?,
            PortRange::NoPorts => f.write_str("none")?,
            PortRange::Port(p) => write!(f, "{p}")?,
            PortRange::Range(l, h) => write!(f, "{l}-{h}")?,
            PortRange::PortSet(set) => join(f, set.iter())?,
        }
        f.write_str(",hosts:")?;
        match (self.any_host, self.hosts.is_empty()) {
            (true, _) => f.write_str("any")?,
            (false, true) => f.write_str("none")?,
            (false, false) => join(f, self.hosts.iter())?,
        }
        match self.exact {
            true => f.write_str(",match:exact"),
            false => Ok(()),
        }
    }
}

fn join<T: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    items: impl Iterator<Item = T>,
) -> fmt::Result {
    for (i, item) in items.enumerate() {
        if i > 0 {
            f.write_str("+")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

/// Whether `name` is `suffix` or ends with `.` and it; both lower-case.
fn under(name: &str, suffix: &str) -> bool {
    name == suffix
        || name
            .strip_suffix(suffix)
            .is_some_and(|head| head.ends_with('.'))
}

/// `name` as it is checked and looked up: one trailing dot dropped, lower
/// case, and a valid host name ([`valid_host`]); None otherwise.
pub fn normalize(name: &str) -> Option<String> {
    let name = name.strip_suffix('.').unwrap_or(name);
    valid_host(name).then(|| name.to_ascii_lowercase())
}

/// A DNS host name: labels of 1-63 letters, digits and `-` (not at either end),
/// at most [`MAX_HOST`] in all, and not all digits and dots.
pub fn valid_host(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_HOST
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && name.parse::<Ipv4Addr>().is_err()
}

fn parse_protocol(value: &str) -> Result<Protocol, &'static str> {
    match value {
        "tcp" => Ok(Protocol::Tcp),
        "udp" => Ok(Protocol::Udp),
        _ => Err("netdest: proto is tcp or udp"),
    }
}

fn parse_addrs(value: &str) -> Result<AddrRange, &'static str> {
    match value {
        "any" => return Ok(AddrRange::Any),
        "none" => return Ok(AddrRange::NoAddrs),
        _ => {}
    }
    let parts: Vec<&str> = value.split('+').collect();
    if parts.len() > MAX_SET_SIZE {
        return Err("netdest: more than 8 addresses");
    }
    if let [one] = parts.as_slice()
        && let Some((addr, prefix)) = one.split_once('/')
    {
        let addr = addr.parse().map_err(|_| "netdest: a bad address")?;
        let prefix: u8 = prefix.parse().map_err(|_| "netdest: a bad prefix")?;
        if prefix > 32 {
            return Err("netdest: a prefix over 32");
        }
        return Ok(AddrRange::Subnet(addr, prefix));
    }
    let addrs: Result<Vec<Ipv4Addr>, _> = parts.iter().map(|p| p.parse()).collect();
    let addrs = addrs.map_err(|_| "netdest: a bad address (one subnet, or up to 8 addresses)")?;
    Ok(match addrs.as_slice() {
        [one] => AddrRange::Addr(*one),
        _ => AddrRange::AddrSet(addrs),
    })
}

fn parse_ports(value: &str) -> Result<PortRange, &'static str> {
    match value {
        "any" => return Ok(PortRange::Any),
        "none" => return Ok(PortRange::NoPorts),
        _ => {}
    }
    if let Some((low, high)) = value.split_once('-') {
        let low: u16 = low.parse().map_err(|_| "netdest: a bad port")?;
        let high: u16 = high.parse().map_err(|_| "netdest: a bad port")?;
        if low > high {
            return Err("netdest: a port range runs backwards");
        }
        return Ok(PortRange::Range(low, high));
    }
    let parts: Vec<&str> = value.split('+').collect();
    if parts.len() > MAX_SET_SIZE {
        return Err("netdest: more than 8 ports");
    }
    let ports: Result<Vec<u16>, _> = parts.iter().map(|p| p.parse()).collect();
    let ports = ports.map_err(|_| "netdest: a bad port (one range, or up to 8 ports)")?;
    Ok(match ports.as_slice() {
        [one] => PortRange::Port(*one),
        _ => PortRange::PortSet(ports),
    })
}

fn parse_hosts(value: &str) -> Result<Vec<String>, &'static str> {
    if value == "none" {
        return Ok(Vec::new());
    }
    let hosts: Vec<&str> = value.split('+').collect();
    if hosts.len() > MAX_HOSTS {
        return Err("netdest: more than 8 hosts");
    }
    hosts
        .iter()
        .map(|host| match valid_host(host) && host.contains('.') {
            true => Ok(host.to_ascii_lowercase().to_string()),
            // A one-label suffix such as `com` would grant a whole top-level domain.
            false => Err("netdest: a host is not a name with a dot in it"),
        })
        .collect()
}
