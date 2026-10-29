//! `kai-gen 1` (ADR 0011 §5): one numbered list of installed packages,
//! `/system/pkg/gen/<n>`.
//!
//! ```text
//! kai-gen 1
//! parent 41
//! time 2026-11-02T10:31:00Z
//! why actlog 4812
//! pkg notes 2.1.0 9f42...8b1d explicit grants 17 18
//! pkg spell-en 1.2.3 1c0e...77aa dep
//! ```
//!
//! Generation 1 has parent 0. `why` is one line of text; `grants` names what the
//! package was granted (`kai-grants 1` ids, P2 task 12), and may be absent.

use crate::version::Version;
use crate::{Sha, hex, unhex};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Write;

/// Seconds since 1970-01-01T00:00:00Z, written `YYYY-MM-DDTHH:MM:SSZ`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Time(pub u64);

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

impl Time {
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
        let (y, m, d) = civil_from_days(self.0 / 86_400);
        let s = self.0 % 86_400;
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

    /// The time `pkg` works at: `date` (seconds since 1970 UTC, the time since
    /// boot already in it), never before the second after `newest`, the newest
    /// generation's time, which is all there is without a date.
    pub fn at(date: Option<u64>, newest: Time) -> Time {
        Time(date.unwrap_or(0).max(newest.0.saturating_add(1)))
    }
}

/// One installed package of a generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub version: Version,
    pub sha: Sha,
    /// Named by the user (or chosen for them), not only pulled in as a dependency.
    pub explicit: bool,
    pub grants: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    pub parent: u64,
    pub time: Time,
    pub why: String,
    pub packages: Vec<Installed>,
}

fn word(w: &str) -> bool {
    !w.is_empty()
        && w.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'*'))
}

impl Generation {
    pub fn format(&self) -> String {
        let mut out = String::from("kai-gen 1\n");
        let _ = writeln!(out, "parent {}", self.parent);
        let _ = writeln!(out, "time {}", self.time.format());
        let _ = writeln!(out, "why {}", self.why);
        for p in &self.packages {
            let kind = if p.explicit { "explicit" } else { "dep" };
            let _ = write!(out, "pkg {} {} {} {kind}", p.name, p.version, hex(&p.sha));
            if !p.grants.is_empty() {
                out.push_str(" grants");
                for g in &p.grants {
                    out.push(' ');
                    out.push_str(g);
                }
            }
            out.push('\n');
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let body = text
            .strip_suffix('\n')
            .ok_or("a generation ends without a newline")?;
        let mut lines = body.split('\n');
        if lines.next() != Some("kai-gen 1") {
            return Err("not a kai-gen 1 generation".into());
        }
        let mut line = |key: &str| -> Result<String, String> {
            lines
                .next()
                .and_then(|l| l.strip_prefix(key))
                .and_then(|r| r.strip_prefix(' '))
                .map(ToString::to_string)
                .ok_or_else(|| alloc::format!("a generation has no {key} line"))
        };
        let parent = crate::version::number(&line("parent")?)?;
        let time = Time::parse(&line("time")?)?;
        let why = line("why")?;
        if why.is_empty() || why.chars().any(char::is_control) {
            return Err("a why line is empty or holds a control character".into());
        }
        let mut packages: Vec<Installed> = Vec::new();
        for l in lines {
            let words: Vec<&str> = l.split(' ').collect();
            let (name, version, sha, kind, rest) = match words.as_slice() {
                ["pkg", name, version, sha, kind, rest @ ..] => {
                    (*name, *version, *sha, *kind, rest)
                }
                _ => return Err(alloc::format!("a generation line {l:?} does not parse")),
            };
            crate::header::valid_name(name)?;
            if packages.iter().any(|p| p.name == name) {
                return Err(alloc::format!("{name} appears twice in a generation"));
            }
            let explicit = match kind {
                "explicit" => true,
                "dep" => false,
                _ => return Err(alloc::format!("{name} is neither explicit nor dep")),
            };
            let grants = match rest {
                [] => Vec::new(),
                ["grants", grants @ ..] if !grants.is_empty() && grants.iter().all(|g| word(g)) => {
                    grants.iter().map(ToString::to_string).collect()
                }
                _ => return Err(alloc::format!("{name}'s grants do not parse")),
            };
            packages.push(Installed {
                name: name.into(),
                version: Version::parse(version)?,
                sha: unhex(sha).ok_or("a package hash is not 64 hex digits")?,
                explicit,
                grants,
            });
        }
        Ok(Generation {
            parent,
            time,
            why,
            packages,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_time_is_the_date_never_before_the_newest_generation() {
        let date = Time::parse("2026-10-28T22:00:00Z").unwrap();
        // A generation made from the date earlier in the boot: a minute later
        // the time is a minute later, not that plus the boot's uptime again.
        assert_eq!(Time::at(Some(date.0 + 60), date), Time(date.0 + 60));
        // A date behind the newest generation (a clock set back) does not
        // move the time back.
        assert_eq!(Time::at(Some(date.0 - 86_400), date), Time(date.0 + 1));
        assert_eq!(Time::at(None, date), Time(date.0 + 1));
        assert_eq!(Time::at(None, Time(u64::MAX)), Time(u64::MAX));
    }

    fn sample() -> Generation {
        Generation {
            parent: 41,
            time: Time::parse("2026-11-02T10:31:00Z").unwrap(),
            why: "actlog 4812".into(),
            packages: vec![
                Installed {
                    name: "notes".into(),
                    version: Version::new(2, 1, 0),
                    sha: crate::sha256(b"notes"),
                    explicit: true,
                    grants: vec!["17".into(), "18".into()],
                },
                Installed {
                    name: "spell-en".into(),
                    version: Version::new(1, 2, 3),
                    sha: crate::sha256(b"spell-en"),
                    explicit: false,
                    grants: vec![],
                },
            ],
        }
    }

    #[test]
    fn a_generation_round_trips() {
        let g = sample();
        let text = g.format();
        assert!(text.contains("pkg notes 2.1.0 "));
        assert!(text.contains(" explicit grants 17 18\n"));
        assert_eq!(Generation::parse(&text).unwrap(), g);
    }

    #[test]
    fn times_round_trip_and_reject_impossible_dates() {
        for t in [
            "1970-01-01T00:00:00Z",
            "2026-11-02T10:31:00Z",
            "2028-02-29T23:59:59Z",
        ] {
            assert_eq!(Time::parse(t).unwrap().format(), t);
        }
        assert_eq!(Time::parse("1970-01-02T00:00:01Z"), Ok(Time(86_401)));
        for bad in [
            "2026-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-11-02T24:00:00Z",
            "2026-11-02 10:31:00Z",
            "1969-12-31T23:59:59Z",
            "2026-11-02T10:31:00",
        ] {
            assert!(Time::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn hostile_generations_are_refused() {
        let good = sample().format();
        let swap = |from: &str, to: &str| good.replacen(from, to, 1);
        let cases = [
            good.trim_end().to_string(),
            swap("kai-gen 1", "kai-gen 2"),
            swap("parent 41", "parent -1"),
            swap("parent 41", "parent 041"),
            swap("why actlog 4812", "why "),
            swap(" explicit grants", " pinned grants"),
            swap(" grants 17 18", " grants"),
            swap(" grants 17 18", " grants 1\u{7}"),
            swap("pkg spell-en", "pkg notes"),
            swap("pkg spell-en", "pkg fs"),
            swap("2.1.0", "2.1"),
            alloc::format!("{good}extra line\n"),
            good.replacen("time 2026-11-02T10:31:00Z\n", "", 1),
        ];
        for case in &cases {
            assert!(Generation::parse(case).is_err(), "{case:?}");
        }
    }
}
