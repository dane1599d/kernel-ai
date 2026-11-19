//! The kernel's `NetDest` records (ADR 0001 D2, P5 task 1): what each per-app
//! destination allows ([`netdest::Dest`]), and the addresses it resolved with
//! when each stops counting. A `NetDest` handle's object is a record's id; the
//! fixed destinations below [`FIRST`] (`kabi::handle::netdest`) are not here.
//!
//! - A resolve on a handle answers only a name under its record's suffixes.
//! - A connect (task 2) reaches only the record's literal ranges, or an address
//!   the record itself resolved, within that answer's TTL ([`may_connect`]).
//! - A record is never widened: a narrower one is made from it, and both stay
//!   ([`narrow`]). Records live until the table fills; then those nothing
//!   names any more are dropped ([`create`]), and their slots are used again.
//!   Named means: by a handle in a table, by one moving between tables in a
//!   queued call or reply ([`transit`]), or made in the last second, which
//!   covers the step from a new record to the handle that names it.
//! - An id carries its slot's generation, which moves on each time the slot
//!   takes a new record, so a handle to a dropped record never names the next
//!   one there: it names nothing (audit K #1).
//! - A `hosts:any` record (the shell's ceiling, from its plan line alone) stays
//!   with the process it was made for: its narrowings name exact hosts only,
//!   never a suffix or `hosts:any` ([`narrow`]), and no handle to it is
//!   moved ([`any_host`], which the move checks without this table's lock).
//! - An agent's record carries its day's bytes (P5 task 14, [`Meter`]): every
//!   record narrowed from it shares that one meter, so a narrowing never buys
//!   a fresh budget, and every stream connected on it counts against it.

use crate::hpet;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use netdest::{DayMeter, Dest, Protocol};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

/// The first record's object id; the fixed destinations sit below it.
pub const FIRST: u64 = 16;
/// Records at most.
const MAX_RECORDS: usize = 256;
/// Resolved addresses one record keeps at most; the oldest go first.
const MAX_RESOLVED: usize = 32;
/// Bits of an id below its generation: the slot.
const SLOT_BITS: u32 = 8;
const _: () = assert!(MAX_RECORDS == 1 << SLOT_BITS);
/// A record this young is never dropped: the step from [`create`] to the
/// handle that names it, preempted at the worst moment, is far shorter.
const FRESH_NS: u64 = 1_000_000_000;

/// An agent destination's bytes a day, shared by every record narrowed from
/// it and every stream on them.
pub type Meter = Arc<Mutex<DayMeter>>;

struct Record {
    /// Its slot's generation when it was made: part of its id.
    generation: u32,
    /// When it was made (HPET).
    born: u64,
    dest: Dest,
    /// Addresses it resolved, and the HPET time each stops counting.
    resolved: Vec<(Ipv4Addr, u64)>,
    /// An agent's day's bytes ([`Meter`]); none for every other record.
    meter: Option<Meter>,
}

/// Today, as the meters count days: whole days of the wall clock since 1970.
/// With no wall clock it is always day 0, so a meter never turns over and
/// buys no more bytes.
pub fn today() -> u32 {
    crate::clock::unix_seconds()
        .and_then(|s| u32::try_from(s / 86_400).ok())
        .unwrap_or(0)
}

/// A meter of `mib` MiB a day, from today.
pub fn meter(mib: u32) -> Meter {
    Arc::new(Mutex::new(DayMeter::from_mib(mib, today())))
}

/// `f` on `meter`, with interrupts off as every lock here is taken.
pub fn with_meter<R>(meter: &Meter, f: impl FnOnce(&mut DayMeter) -> R) -> R {
    without_interrupts(|| f(&mut meter.lock()))
}

static RECORDS: Mutex<Vec<Option<Record>>> = Mutex::new(Vec::new());

/// Which records are `hosts:any`, one bit each, kept beside [`RECORDS`] so a
/// handle move can ask under the handle table's lock: [`create`] takes the
/// handle tables' locks inside this table's, so the other order would deadlock.
static ANY_HOST: [AtomicU64; MAX_RECORDS / 64] = [const { AtomicU64::new(0) }; MAX_RECORDS / 64];
/// Each slot's generation, moved on as it takes a new record.
static GENERATION: [AtomicU32; MAX_RECORDS] = [const { AtomicU32::new(0) }; MAX_RECORDS];
/// Handles to each slot in transit between tables ([`transit`]).
static TRANSIT: [AtomicU32; MAX_RECORDS] = [const { AtomicU32::new(0) }; MAX_RECORDS];

/// A handle to `object` left a table to move to another (`up`), or reached
/// it or was dropped on the way: while any is moving, its record stays.
/// Counted by slot, whatever the generation, so each count is undone.
pub fn transit(object: u64, up: bool) {
    let Some((at, _)) = slot_of(object) else {
        return;
    };
    if up {
        TRANSIT[at].fetch_add(1, Ordering::SeqCst);
    } else {
        let _ = TRANSIT[at].try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
    }
}

fn mark_any_host(at: usize, any: bool) {
    let bit = 1u64 << (at % 64);
    match any {
        true => ANY_HOST[at / 64].fetch_or(bit, Ordering::SeqCst),
        false => ANY_HOST[at / 64].fetch_and(!bit, Ordering::SeqCst),
    };
}

/// Whether `object` is a `hosts:any` record: a handle to it is never moved to
/// another process (`handle::take_for`).
pub fn any_host(object: u64) -> bool {
    slot_of(object)
        .is_some_and(|(at, _)| ANY_HOST[at / 64].load(Ordering::SeqCst) & (1u64 << (at % 64)) != 0)
}

/// The slot and generation an id packs, for any id at or above [`FIRST`].
fn slot_of(object: u64) -> Option<(usize, u32)> {
    let raw = object.checked_sub(FIRST)?;
    let at = (raw & (MAX_RECORDS as u64 - 1)) as usize;
    let generation = u32::try_from(raw >> SLOT_BITS).ok()?;
    Some((at, generation))
}

fn id(at: usize, generation: u32) -> u64 {
    FIRST + (u64::from(generation) << SLOT_BITS) + at as u64
}

/// The record `object` names: its slot's, when the generation matches.
fn record_of(records: &[Option<Record>], object: u64) -> Option<&Record> {
    let (at, generation) = slot_of(object)?;
    records
        .get(at)?
        .as_ref()
        .filter(|r| r.generation == generation)
}

fn with<R>(f: impl FnOnce(&mut Vec<Option<Record>>) -> R) -> R {
    without_interrupts(|| f(&mut RECORDS.lock()))
}

/// A new record for `dest`: its object id. When the table is full, records no
/// live handle names (`live`) are dropped first; None if it is still full.
pub fn create(dest: Dest, live: impl Fn(u64) -> bool) -> Option<u64> {
    create_metered(dest, live, None)
}

/// [`create`], the record counting its bytes on `meter` when given.
pub fn create_metered(dest: Dest, live: impl Fn(u64) -> bool, meter: Option<Meter>) -> Option<u64> {
    with(|records| {
        let free = records.iter().position(Option::is_none);
        let at = match free {
            Some(at) => at,
            None if records.len() < MAX_RECORDS => {
                records.push(None);
                records.len() - 1
            }
            None => {
                let now = hpet::nanos();
                for (i, slot) in records.iter_mut().enumerate() {
                    let Some(record) = slot else {
                        continue;
                    };
                    // The transit count first, then the tables, then whether
                    // any handle moved meanwhile: one of the three sees a
                    // handle that moved while it looked.
                    let before = crate::handle::moves();
                    let named = TRANSIT[i].load(Ordering::SeqCst) != 0
                        || live(id(i, record.generation))
                        || crate::handle::moves() != before
                        || now.saturating_sub(record.born) < FRESH_NS;
                    if !named {
                        *slot = None;
                    }
                }
                records.iter().position(Option::is_none)?
            }
        };
        let generation = GENERATION[at]
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        mark_any_host(at, dest.any_host);
        records[at] = Some(Record {
            generation,
            born: hpet::nanos(),
            dest,
            resolved: Vec::new(),
            meter,
        });
        Some(id(at, generation))
    })
}

/// The destination `object` names, if it is a record.
pub fn dest(object: u64) -> Option<Dest> {
    with(|records| record_of(records, object).map(|r| r.dest.clone()))
}

/// The meter `object`'s record counts on, if it is an agent's.
pub fn meter_of(object: u64) -> Option<Meter> {
    with(|records| record_of(records, object)?.meter.clone())
}

/// Whether a resolve of `name` on `object` may be answered.
pub fn allows_name(object: u64, name: &str) -> bool {
    dest(object).is_some_and(|d| d.allows_name(name))
}

/// Keeps `addresses`, which `object` resolved, for `ttl_s` seconds.
pub fn resolved(object: u64, addresses: &[Ipv4Addr], ttl_s: u64) {
    let Some((at, generation)) = slot_of(object) else {
        return;
    };
    let until = hpet::nanos().saturating_add(ttl_s.saturating_mul(1_000_000_000));
    let now = hpet::nanos();
    with(|records| {
        let Some(Some(record)) = records.get_mut(at) else {
            return;
        };
        if record.generation != generation {
            return;
        }
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
    let now = hpet::nanos();
    with(|records| {
        let Some(record) = record_of(records, object) else {
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
/// everyone or a group, and a private network (audit K #17): 10/8,
/// 172.16/12, 192.168/16, the carrier's 100.64/10, 0/8 and 240/4. An answer
/// pointing there is how DNS rebinding reaches what the app was never given,
/// and a probe of the person's LAN and the VM host on its ports.
///
/// One exception, for the boot cases' own network: with `online=test` on
/// the kernel command line, QEMU's user network 10.0.2.0/24 (the mock DNS's
/// fixtures and the HTTPS mock) is not refused. A release image never
/// carries that word (`xtask::image::slots::build_release`).
fn special(addr: Ipv4Addr) -> bool {
    netdest::special(addr, test_network())
}

/// Whether a name's answer may lead to `addr`: RESOLVE_ON hands an app only
/// such addresses, and keeps only them for its connects.
pub fn answerable(addr: Ipv4Addr) -> bool {
    !special(addr)
}

/// Whether the boot runs on the boot cases' test network (`online=test`).
fn test_network() -> bool {
    crate::cmdline()
        .split_whitespace()
        .any(|word| word == "online=test")
}

/// A record for `narrower`, made from `object`'s, if `object`'s covers it:
/// its id. The addresses `object` resolved do not carry over.
pub fn narrow(object: u64, narrower: Dest, live: impl Fn(u64) -> bool) -> Result<u64, kabi::Error> {
    let wider = dest(object).ok_or(kabi::Error::Invalid)?;
    // `hosts:any` is made only from the shell's plan line, never by narrowing,
    // and narrows only to exact hosts: a suffix such as `co.uk` would carry
    // much of it away (C on #760).
    let suffix_of_any = wider.any_host && !narrower.exact;
    if narrower.any_host || suffix_of_any || !narrower.within(&wider) {
        return Err(kabi::Error::Permission);
    }
    // The narrower record counts on the wider one's meter: never a fresh day.
    create_metered(narrower, live, meter_of(object)).ok_or(kabi::Error::NoMemory)
}
