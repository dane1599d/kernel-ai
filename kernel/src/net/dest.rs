//! The kernel's `NetDest` records (ADR 0001 D2, P5 task 1): what each per-app
//! destination allows ([`netdest::Dest`]), and the addresses it resolved with
//! when each stops counting. A `NetDest` handle's object is a record's id; the
//! fixed destinations below [`FIRST`] (`kabi::handle::netdest`) are not here.
//!
//! - A resolve on a handle answers only a name under its record's suffixes.
//! - A connect (task 2) reaches only the record's literal ranges, or an address
//!   the record itself resolved, within that answer's TTL ([`may_connect`]).
//! - A record is never widened: a narrower one is made from it, and both stay
//!   ([`narrow`]). Records live until the table fills; then those no handle
//!   names any more are dropped ([`create`]), and their ids are used again.
//!   That is safe because no handle names a dropped one: a handle closed
//!   since can never come back to the new record.

use crate::hpet;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use netdest::{Dest, Protocol};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

/// The first record's object id; the fixed destinations sit below it.
pub const FIRST: u64 = 16;
/// Records at most.
const MAX_RECORDS: usize = 256;
/// Resolved addresses one record keeps at most; the oldest go first.
const MAX_RESOLVED: usize = 32;

struct Record {
    dest: Dest,
    /// Addresses it resolved, and the HPET time each stops counting.
    resolved: Vec<(Ipv4Addr, u64)>,
}

static RECORDS: Mutex<Vec<Option<Record>>> = Mutex::new(Vec::new());

fn index(object: u64) -> Option<usize> {
    usize::try_from(object.checked_sub(FIRST)?).ok()
}

fn with<R>(f: impl FnOnce(&mut Vec<Option<Record>>) -> R) -> R {
    without_interrupts(|| f(&mut RECORDS.lock()))
}

/// A new record for `dest`: its object id. When the table is full, records no
/// live handle names (`live`) are dropped first; None if it is still full.
pub fn create(dest: Dest, live: impl Fn(u64) -> bool) -> Option<u64> {
    with(|records| {
        let free = records.iter().position(Option::is_none);
        let at = match free {
            Some(at) => at,
            None if records.len() < MAX_RECORDS => {
                records.push(None);
                records.len() - 1
            }
            None => {
                for (i, record) in records.iter_mut().enumerate() {
                    if record.is_some() && !live(FIRST + i as u64) {
                        *record = None;
                    }
                }
                records.iter().position(Option::is_none)?
            }
        };
        records[at] = Some(Record {
            dest,
            resolved: Vec::new(),
        });
        Some(FIRST + at as u64)
    })
}

/// The destination `object` names, if it is a record.
pub fn dest(object: u64) -> Option<Dest> {
    let at = index(object)?;
    with(|records| records.get(at)?.as_ref().map(|r| r.dest.clone()))
}

/// Whether a resolve of `name` on `object` may be answered.
pub fn allows_name(object: u64, name: &str) -> bool {
    dest(object).is_some_and(|d| d.allows_name(name))
}

/// Keeps `addresses`, which `object` resolved, for `ttl_s` seconds.
pub fn resolved(object: u64, addresses: &[Ipv4Addr], ttl_s: u64) {
    let Some(at) = index(object) else {
        return;
    };
    let until = hpet::nanos().saturating_add(ttl_s.saturating_mul(1_000_000_000));
    let now = hpet::nanos();
    with(|records| {
        let Some(Some(record)) = records.get_mut(at) else {
            return;
        };
        record
            .resolved
            .retain(|(addr, ends)| *ends > now && !addresses.contains(addr));
        for &addr in addresses {
            if record.resolved.len() == MAX_RESOLVED {
                record.resolved.remove(0);
            }
            record.resolved.push((addr, until));
        }
    });
}

/// Whether a connect on `object` may reach `addr`:`port` over `protocol`: in
/// its literal ranges, or an address it resolved whose TTL has not run out,
/// on one of its ports.
pub fn may_connect(object: u64, protocol: Protocol, addr: Ipv4Addr, port: u16) -> bool {
    let Some(at) = index(object) else {
        return false;
    };
    let now = hpet::nanos();
    with(|records| {
        let Some(Some(record)) = records.get(at) else {
            return false;
        };
        record.dest.allows_literal(protocol, addr, port)
            || (record.dest.allows_resolved_port(protocol, port)
                && !special(addr)
                && record
                    .resolved
                    .iter()
                    .any(|(a, ends)| *a == addr && *ends > now))
    })
}

/// An address a name's answer must not lead to unless the record names it
/// literally (lane C's review of task 1): this machine, the link, nothing,
/// everyone or a group. An answer pointing there is how DNS rebinding reaches
/// what the app was never given.
fn special(addr: Ipv4Addr) -> bool {
    addr.is_loopback()
        || addr.is_link_local()
        || addr.is_unspecified()
        || addr.is_broadcast()
        || addr.is_multicast()
}

/// A record for `narrower`, made from `object`'s, if `object`'s covers it:
/// its id. The addresses `object` resolved do not carry over.
pub fn narrow(object: u64, narrower: Dest, live: impl Fn(u64) -> bool) -> Result<u64, kabi::Error> {
    let wider = dest(object).ok_or(kabi::Error::Invalid)?;
    if !narrower.within(&wider) {
        return Err(kabi::Error::Permission);
    }
    create(narrower, live).ok_or(kabi::Error::NoMemory)
}
