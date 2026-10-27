//! `made=` times: seconds since 1970-01-01T00:00:00Z, written
//! `YYYY-MM-DDTHH:MM:SSZ`, the shape `kai-gen 1` uses (kaipkg's
//! `generation::Time`, which can move onto this one).

use alloc::string::String;
use core::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time(pub u64);

/// The latest time written: 9999-12-31T23:59:59Z.
const LAST: u64 = 253_402_300_799;

impl Time {
    /// From nanoseconds since 1970, as the shell's clock gives them.
    pub fn from_ns(ns: u64) -> Self {
        Time((ns / 1_000_000_000).min(LAST))
    }

    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let b = text.as_bytes();
        let shape = b.len() == 20
            && b.iter().enumerate().all(|(i, c)| match i {
                4 | 7 => *c == b'-',
                10 => *c == b'T',
                13 | 16 => *c == b':',
                19 => *c == b'Z',
                _ => c.is_ascii_digit(),
            });
        if !shape {
            return Err("a time is not YYYY-MM-DDTHH:MM:SSZ");
        }
        let n = |from: usize, to: usize| -> u64 { text[from..to].parse().unwrap_or(u64::MAX) };
        let (y, m, d, hh, mm, ss) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19));
        let valid = y >= 1970
            && (1..=12).contains(&m)
            && (1..=days_in_month(y, m)).contains(&d)
            && hh < 24
            && mm < 60
            && ss < 60;
        if !valid {
            return Err("a time names no real moment");
        }
        Ok(Time(
            days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss,
        ))
    }

    pub fn format(self) -> String {
        let (y, m, d) = civil_from_days(self.0.min(LAST) / 86_400);
        let s = self.0.min(LAST) % 86_400;
        let mut out = String::new();
        let _ = write!(
            out,
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            s / 3600,
            s / 60 % 60,
            s % 60
        );
        out
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`), for years 1970-9999.
fn days_from_civil(y: u64, m: u64, d: u64) -> u64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: u64) -> (u64, u64, u64) {
    let z = z + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u64::from(m <= 2);
    (y, m, d)
}

fn days_in_month(y: u64, m: u64) -> u64 {
    match m {
        2 if (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
