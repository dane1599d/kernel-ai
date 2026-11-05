// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.
// Modified for Kernel AI, 2026: IPv4 (`core::net::Ipv4Addr`) in place of
// Tock's IPv6 `IPAddr`; sets in a `Vec` of at most eight; `covers`, the
// subset check a handle that only shrinks needs; no visibility capabilities,
// since the kernel alone holds these.

//! Address and port ranges for a `NetDest` (ADR 0001 D2), ported from Tock's
//! `capsules/extra/src/net/network_capabilities.rs`.

use alloc::vec::Vec;
use core::net::Ipv4Addr;

/// The most addresses or ports a set holds, as in Tock.
pub const MAX_SET_SIZE: usize = 8;

/// The addresses a destination may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddrRange {
    Any,
    NoAddrs,
    AddrSet(Vec<Ipv4Addr>),
    Addr(Ipv4Addr),
    /// An address and a prefix length, at most 32.
    Subnet(Ipv4Addr, u8),
}

impl AddrRange {
    pub fn is_addr_valid(&self, addr: Ipv4Addr) -> bool {
        self.spans()
            .iter()
            .any(|&(low, high)| (low..=high).contains(&u32::from(addr)))
    }

    /// Whether every address `inner` allows, this allows too.
    pub fn covers(&self, inner: &AddrRange) -> bool {
        covered(&self.spans(), &inner.spans())
    }

    /// The range as inclusive spans of addresses.
    fn spans(&self) -> Vec<(u32, u32)> {
        match self {
            AddrRange::Any => alloc::vec![(0, u32::MAX)],
            AddrRange::NoAddrs => Vec::new(),
            AddrRange::AddrSet(addrs) => addrs
                .iter()
                .map(|&a| (u32::from(a), u32::from(a)))
                .collect(),
            AddrRange::Addr(a) => alloc::vec![(u32::from(*a), u32::from(*a))],
            AddrRange::Subnet(a, prefix) => {
                let mask = match prefix {
                    0 => 0,
                    p => u32::MAX << (32 - u32::from((*p).min(32))),
                };
                let low = u32::from(*a) & mask;
                alloc::vec![(low, low | !mask)]
            }
        }
    }
}

/// The remote ports a destination may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortRange {
    Any,
    NoPorts,
    PortSet(Vec<u16>),
    Range(u16, u16),
    Port(u16),
}

impl PortRange {
    pub fn is_port_valid(&self, port: u16) -> bool {
        match self {
            PortRange::Any => true,
            PortRange::NoPorts => false,
            PortRange::PortSet(allowed) => allowed.contains(&port),
            PortRange::Range(low, high) => *low <= port && port <= *high,
            PortRange::Port(allowed) => port == *allowed,
        }
    }

    /// Whether every port `inner` allows, this allows too.
    pub fn covers(&self, inner: &PortRange) -> bool {
        covered(&self.spans(), &inner.spans())
    }

    fn spans(&self) -> Vec<(u32, u32)> {
        match self {
            PortRange::Any => alloc::vec![(0, u32::from(u16::MAX))],
            PortRange::NoPorts => Vec::new(),
            PortRange::PortSet(ports) => ports
                .iter()
                .map(|&p| (u32::from(p), u32::from(p)))
                .collect(),
            PortRange::Range(low, high) if low <= high => {
                alloc::vec![(u32::from(*low), u32::from(*high))]
            }
            PortRange::Range(..) => Vec::new(),
            PortRange::Port(p) => alloc::vec![(u32::from(*p), u32::from(*p))],
        }
    }
}

/// Whether the union of `outer` holds every span of `inner`.
fn covered(outer: &[(u32, u32)], inner: &[(u32, u32)]) -> bool {
    let mut merged: Vec<(u32, u32)> = outer.to_vec();
    merged.sort_unstable();
    let mut joined: Vec<(u32, u32)> = Vec::new();
    for (low, high) in merged {
        match joined.last_mut() {
            Some(last) if low <= last.1.saturating_add(1) => last.1 = last.1.max(high),
            _ => joined.push((low, high)),
        }
    }
    inner
        .iter()
        .all(|&(low, high)| joined.iter().any(|&(l, h)| l <= low && high <= h))
}
