//! A POSIX TZ string, as a TZif file's footer gives the rules after its last
//! transition (RFC 8536 §3.3, with its version 3 extensions): `EST5EDT,M3.2.0,
//! M11.1.0`, `<+0545>-5:45`, `AEST-10AEDT,M10.1.0,M4.1.0/3`.

use crate::Error;
use crate::civil::{DAY, days_from_civil, is_leap, month_days, weekday};
use alloc::string::String;

/// The local time type a rule gives at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    /// Seconds east of UTC.
    pub offset: i32,
    pub dst: bool,
    pub abbr: String,
}

/// A zone's rule: standard time alone, or standard and daylight saving time
/// with the dates they change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub std: Kind,
    pub dst: Option<(Kind, Change, Change)>,
}

/// When daylight saving time starts or ends in a year: a day, and the local
/// time on it (in the time that is ending), in seconds; it may be negative or
/// past 24 hours (version 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub day: Day,
    pub time: i32,
}

/// The day of a [`Change`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Day {
    /// `Jn`: day 1 to 365, never counting 29 February.
    Julian(u16),
    /// `n`: day 0 to 365, counting 29 February.
    Zero(u16),
    /// `Mm.w.d`: weekday `d` (0 Sunday) of week `w` (5 the last) of month `m`.
    Month(u8, u8, u8),
}

impl Rule {
    /// Parses a POSIX TZ string.
    pub fn parse(text: &str) -> Result<Rule, Error> {
        let mut p = Parser {
            s: text.as_bytes(),
            at: 0,
        };
        let std_abbr = p.name()?;
        let std_offset = -p.offset(24)?;
        let std = Kind {
            offset: std_offset,
            dst: false,
            abbr: std_abbr,
        };
        if p.done() {
            return Ok(Rule { std, dst: None });
        }
        let dst_abbr = p.name()?;
        let dst_offset = match p.peek() {
            Some(b',') | None => std_offset + 3600,
            Some(_) => -p.offset(24)?,
        };
        let dst = Kind {
            offset: dst_offset,
            dst: true,
            abbr: dst_abbr,
        };
        // No dates: POSIX leaves them to the system; the US rule since 2007.
        let (start, end) = if p.done() {
            let at = 2 * 3600;
            let start = Change {
                day: Day::Month(3, 2, 0),
                time: at,
            };
            let end = Change {
                day: Day::Month(11, 1, 0),
                time: at,
            };
            (start, end)
        } else {
            p.expect(b',')?;
            let start = p.change()?;
            p.expect(b',')?;
            (start, p.change()?)
        };
        if !p.done() {
            return Err(Error::Rule);
        }
        Ok(Rule {
            std,
            dst: Some((dst, start, end)),
        })
    }

    /// The local time type at `utc` seconds from the epoch.
    pub fn at(&self, utc: i64) -> &Kind {
        let Some((dst, start, end)) = &self.dst else {
            return &self.std;
        };
        let local = utc.saturating_add(i64::from(self.std.offset));
        let year = crate::civil::DateTime::from_seconds(local).year;
        // Each change is in the local time that is ending.
        let begins = start.local(year) - i64::from(self.std.offset);
        let ends = end.local(year) - i64::from(dst.offset);
        let in_dst = if begins <= ends {
            begins <= utc && utc < ends
        } else {
            // The southern hemisphere: daylight saving time spans the new year.
            utc < ends || begins <= utc
        };
        if in_dst { dst } else { &self.std }
    }
}

impl Change {
    /// Seconds from the epoch, on the wall clock, when this change comes in
    /// `year`.
    fn local(self, year: i32) -> i64 {
        let jan1 = days_from_civil(year, 1, 1);
        let day = match self.day {
            Day::Julian(n) => {
                let n = i64::from(n) - 1;
                jan1 + n + i64::from(is_leap(year) && n >= 59)
            }
            Day::Zero(n) => jan1 + i64::from(n),
            Day::Month(m, w, d) => {
                let first = days_from_civil(year, m, 1);
                let mut day =
                    first + i64::from((7 + d - weekday(first)) % 7) + 7 * i64::from(w - 1);
                while day >= first + i64::from(month_days(year, m)) {
                    day -= 7;
                }
                day
            }
        };
        day * DAY + i64::from(self.time)
    }
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    fn done(&self) -> bool {
        self.at == self.s.len()
    }

    fn expect(&mut self, byte: u8) -> Result<(), Error> {
        if self.peek() != Some(byte) {
            return Err(Error::Rule);
        }
        self.at += 1;
        Ok(())
    }

    /// An abbreviation: three or more letters, or `<...>` of letters,
    /// digits, `+` and `-`.
    fn name(&mut self) -> Result<String, Error> {
        let quoted = self.peek() == Some(b'<');
        if quoted {
            self.at += 1;
        }
        let start = self.at;
        while let Some(c) = self.peek() {
            let ok =
                c.is_ascii_alphabetic() || quoted && (c.is_ascii_digit() || c == b'+' || c == b'-');
            if !ok {
                break;
            }
            self.at += 1;
        }
        let name = &self.s[start..self.at];
        if quoted {
            self.expect(b'>')?;
        }
        if name.len() < 3 {
            return Err(Error::Rule);
        }
        // Only ASCII bytes were taken.
        Ok(String::from(
            core::str::from_utf8(name).map_err(|_| Error::Rule)?,
        ))
    }

    /// `[+-]hh[:mm[:ss]]` in seconds, hours at most `max_hours`.
    fn offset(&mut self, max_hours: i64) -> Result<i32, Error> {
        let negative = match self.peek() {
            Some(b'-') => true,
            Some(b'+') => false,
            _ => return self.unsigned(max_hours),
        };
        self.at += 1;
        let value = self.unsigned(max_hours)?;
        Ok(if negative { -value } else { value })
    }

    fn unsigned(&mut self, max_hours: i64) -> Result<i32, Error> {
        let hours = self.number(3)?;
        let mut seconds = hours * 3600;
        for scale in [60, 1] {
            if self.peek() != Some(b':') {
                break;
            }
            self.at += 1;
            let part = self.number(2)?;
            if part > 59 {
                return Err(Error::Rule);
            }
            seconds += part * scale;
        }
        if hours > max_hours {
            return Err(Error::Rule);
        }
        Ok(seconds as i32)
    }

    /// Up to `digits` decimal digits, at least one.
    fn number(&mut self, digits: usize) -> Result<i64, Error> {
        let start = self.at;
        let mut value = 0i64;
        while let Some(c) = self.peek().filter(u8::is_ascii_digit) {
            if self.at - start == digits {
                return Err(Error::Rule);
            }
            value = value * 10 + i64::from(c - b'0');
            self.at += 1;
        }
        if self.at == start {
            return Err(Error::Rule);
        }
        Ok(value)
    }

    /// `date[/time]`; the time defaults to 02:00 and may be -167 to 167 hours.
    fn change(&mut self) -> Result<Change, Error> {
        let day = match self.peek() {
            Some(b'J') => {
                self.at += 1;
                let n = self.number(3)?;
                if !(1..=365).contains(&n) {
                    return Err(Error::Rule);
                }
                Day::Julian(n as u16)
            }
            Some(b'M') => {
                self.at += 1;
                let m = self.number(2)?;
                self.expect(b'.')?;
                let w = self.number(1)?;
                self.expect(b'.')?;
                let d = self.number(1)?;
                if !(1..=12).contains(&m) || !(1..=5).contains(&w) || d > 6 {
                    return Err(Error::Rule);
                }
                Day::Month(m as u8, w as u8, d as u8)
            }
            _ => {
                let n = self.number(3)?;
                if n > 365 {
                    return Err(Error::Rule);
                }
                Day::Zero(n as u16)
            }
        };
        let time = if self.peek() == Some(b'/') {
            self.at += 1;
            self.offset(167)?
        } else {
            2 * 3600
        };
        Ok(Change { day, time })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civil::DateTime;

    fn utc(y: i32, m: u8, d: u8, h: u8, min: u8) -> i64 {
        DateTime::date(y, m, d).at(h, min, 0).seconds()
    }

    #[test]
    fn a_us_rule_changes_on_the_second_and_first_sundays() {
        let rule = Rule::parse("EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(rule.std.offset, -5 * 3600);
        // 2024-03-10 02:00 EST is 07:00 UTC; 2024-11-03 02:00 EDT is 06:00 UTC.
        assert_eq!(rule.at(utc(2024, 3, 10, 6, 59)).abbr, "EST");
        assert_eq!(rule.at(utc(2024, 3, 10, 7, 0)).abbr, "EDT");
        assert_eq!(rule.at(utc(2024, 11, 3, 5, 59)).offset, -4 * 3600);
        assert_eq!(rule.at(utc(2024, 11, 3, 6, 0)).offset, -5 * 3600);
        assert_eq!(
            rule,
            Rule::parse("EST5EDT").unwrap(),
            "the dates default to the US rule"
        );
    }

    #[test]
    fn quoted_names_minutes_and_hours_past_24_parse() {
        let rule = Rule::parse("<+0545>-5:45").unwrap();
        assert_eq!(rule.std.offset, 5 * 3600 + 45 * 60);
        assert_eq!(rule.std.abbr, "+0545");
        assert!(rule.dst.is_none());
        let rule = Rule::parse("<-03>3<-02>,M3.5.0/-2,M10.5.0/-1").unwrap();
        let (dst, start, _) = rule.dst.unwrap();
        assert_eq!(dst.offset, -2 * 3600, "an hour ahead by default");
        assert_eq!(start.time, -2 * 3600);
        let rule = Rule::parse("EST5EDT,0/0,J365/25").unwrap();
        let (_, start, end) = rule.dst.unwrap();
        assert_eq!(
            (start.day, end.day, end.time),
            (Day::Zero(0), Day::Julian(365), 25 * 3600)
        );
    }

    #[test]
    fn julian_days_skip_29_february_and_zero_based_ones_count_it() {
        let march1 = Change {
            day: Day::Julian(60),
            time: 0,
        };
        assert_eq!(march1.local(2024), utc(2024, 3, 1, 0, 0));
        assert_eq!(march1.local(2023), utc(2023, 3, 1, 0, 0));
        let day59 = Change {
            day: Day::Zero(59),
            time: 0,
        };
        assert_eq!(day59.local(2024), utc(2024, 2, 29, 0, 0));
        let last_sunday = Change {
            day: Day::Month(3, 5, 0),
            time: 3600,
        };
        assert_eq!(last_sunday.local(2024), utc(2024, 3, 31, 1, 0));
    }

    #[test]
    fn broken_strings_are_refused() {
        for bad in [
            "",
            "ES5",
            "EST",
            "EST5EDT,M3.2.0",
            "EST5EDT,M13.1.0,M11.1.0",
            "EST5EDT,M3.6.0,M11.1.0",
            "EST5EDT,M3.2.7,M11.1.0",
            "EST25",
            "EST5:60",
            "<+05",
            "EST5EDT,J0,J365",
            "EST5EDT,366,0",
            "EST5EDT,M3.2.0/168,M11.1.0",
            "EST5x",
            "EST5EDT,M3.2.0,M11.1.0,",
        ] {
            assert_eq!(Rule::parse(bad), Err(Error::Rule), "{bad:?}");
        }
    }
}
