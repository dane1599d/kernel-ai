//! Time zones with daylight saving time (plan L-1, the host half): a zone read
//! from a TZif file, the tz database's compiled form, or from a POSIX TZ string
//! alone, and the conversions between UTC and its local time across its
//! transitions. A local time in a spring-forward gap, or in the hour repeated
//! when the clocks go back, says so ([`Resolved`]).
//!
//! `no_std` with `alloc`, and no dependencies, so the kernel's clock or
//! `desktop` can take it as it is. No tz data is in this crate; its tests
//! build their zones from published transition dates, and read the host's
//! `/usr/share/zoneinfo` where there is one.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod civil;
pub mod rule;
pub mod tzif;

pub use civil::DateTime;
pub use rule::{Kind, Rule};

use alloc::vec;
use alloc::vec::Vec;

/// Why a zone could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The file does not start with `TZif`.
    Magic,
    /// A version byte other than 0, `2`, `3` or `4`.
    Version,
    /// A header's counts that cannot be: no time types, more than 256, or
    /// standard and UT indicators that are not one per type.
    Counts,
    /// The file ends inside its data.
    Truncated,
    /// Transition times out of order.
    Order,
    /// A transition naming a time type there is not.
    Index,
    /// A time type with an impossible offset, flag or abbreviation.
    Kind,
    /// A version 2+ footer that is not `\n<TZ string>\n`.
    Footer,
    /// A TZ string that does not parse.
    Rule,
    /// Larger than [`tzif::MAX_BYTES`].
    TooBig,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let what = match self {
            Error::Magic => "not a TZif file",
            Error::Version => "an unknown TZif version",
            Error::Counts => "impossible TZif header counts",
            Error::Truncated => "the TZif file is cut short",
            Error::Order => "transition times out of order",
            Error::Index => "a transition names no time type",
            Error::Kind => "a bad local time type",
            Error::Footer => "a bad TZif footer",
            Error::Rule => "a bad TZ string",
            Error::TooBig => "the TZif file is too big",
        };
        f.write_str(what)
    }
}

/// A time zone: its transitions, the local time type after each, and the
/// rule that holds after the last one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone {
    /// Transition times, seconds from the epoch in UTC, ascending.
    times: Vec<i64>,
    /// For each transition, the type in `kinds` from then on.
    indices: Vec<u8>,
    /// The local time types; the first holds before the first transition.
    kinds: Vec<Kind>,
    /// From the last transition on; without it the last type holds.
    rule: Option<Rule>,
}

/// What a local wall-clock time is in UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolved {
    /// The usual case: one moment.
    Single(i64),
    /// The clocks went back: the time came twice, first in the earlier
    /// offset (daylight saving time, usually), then in the later.
    Ambiguous { earlier: i64, later: i64 },
    /// The clocks went forward over it: it never came. `transition` is the
    /// moment they did; `shifted` is the time moved on by the gap, as most
    /// systems take it (02:30 on a spring-forward night is 03:30).
    Gap { transition: i64, shifted: i64 },
}

impl Resolved {
    /// One moment for it: the earlier of a repeated time, the shifted one in
    /// a gap.
    pub fn compatible(self) -> i64 {
        match self {
            Resolved::Single(t) => t,
            Resolved::Ambiguous { earlier, .. } => earlier,
            Resolved::Gap { shifted, .. } => shifted,
        }
    }
}

impl Zone {
    /// Reads a TZif file (`/usr/share/zoneinfo/Europe/Berlin`, say).
    pub fn parse(bytes: &[u8]) -> Result<Zone, Error> {
        tzif::parse(bytes)
    }

    /// A zone from a POSIX TZ string alone (`CET-1CEST,M3.5.0,M10.5.0/3`):
    /// its rule for every year, with no history.
    pub fn from_rule(text: &str) -> Result<Zone, Error> {
        let rule = Rule::parse(text)?;
        Ok(Zone {
            times: Vec::new(),
            indices: Vec::new(),
            kinds: vec![rule.std.clone()],
            rule: Some(rule),
        })
    }

    /// The number of transitions the file lists, before its rule takes over.
    pub fn transitions(&self) -> usize {
        self.times.len()
    }

    /// The local time type at `utc` seconds from the epoch. Exact for any
    /// moment of years -2^31 to 2^31; past that, a type and no panic.
    pub fn at(&self, utc: i64) -> &Kind {
        let last = self.times.last().copied();
        if let Some(rule) = &self.rule
            && last.is_none_or(|last| utc >= last)
        {
            return rule.at(utc);
        }
        let i = match self.times.binary_search(&utc) {
            Ok(i) => i,
            Err(0) => return &self.kinds[0],
            Err(i) => i - 1,
        };
        &self.kinds[usize::from(self.indices[i])]
    }

    /// `utc` as the local wall clock, with its time type.
    pub fn local(&self, utc: i64) -> (DateTime, &Kind) {
        let kind = self.at(utc);
        (
            DateTime::from_seconds(utc.saturating_add(i64::from(kind.offset))),
            kind,
        )
    }

    /// The local wall-clock time `wall` in UTC.
    pub fn resolve(&self, wall: DateTime) -> Resolved {
        let local = wall.seconds();
        // The offsets either side; no zone has changed twice within 4 days.
        let before = self.at(local.saturating_sub(2 * civil::DAY)).offset;
        let after = self.at(local.saturating_add(2 * civil::DAY)).offset;
        let mut found: Vec<i64> = [before, after]
            .iter()
            .map(|&o| local - i64::from(o))
            .filter(|&utc| i64::from(self.at(utc).offset) == local - utc)
            .collect();
        found.sort_unstable();
        found.dedup();
        match found[..] {
            [utc] => Resolved::Single(utc),
            [earlier, later] => Resolved::Ambiguous { earlier, later },
            _ if after > before => {
                // In the gap: the change is between these two moments.
                let (mut lo, mut hi) = (local - i64::from(after), local - i64::from(before));
                while hi - lo > 1 {
                    let mid = lo + (hi - lo) / 2;
                    if self.at(mid).offset == before {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                Resolved::Gap {
                    transition: hi,
                    shifted: local - i64::from(before),
                }
            }
            // Two changes within the window: a shape no zone has.
            _ => Resolved::Single(local - i64::from(before)),
        }
    }
}
