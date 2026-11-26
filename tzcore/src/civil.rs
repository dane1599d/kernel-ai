//! Dates on the proleptic Gregorian calendar, as days and seconds from the Unix
//! epoch (1970-01-01, a Thursday). The day arithmetic is the usual era-based
//! one (400-year eras of 146,097 days), exact for every `i64` second the
//! tz data reaches.

/// Seconds in a day.
pub const DAY: i64 = 86_400;

/// A wall-clock date and time: in UTC, or local to a zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DateTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Midnight on `year`-`month`-`day`, with the time set by [`at`](Self::at).
    pub const fn date(year: i32, month: u8, day: u8) -> Self {
        DateTime {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }

    /// This date at `hour`:`minute`:`second`.
    pub const fn at(self, hour: u8, minute: u8, second: u8) -> Self {
        DateTime {
            hour,
            minute,
            second,
            ..self
        }
    }

    /// Seconds from the epoch, reading this as UTC; for a local time, the
    /// seconds of its wall clock ([`crate::Zone::resolve`] takes them).
    pub fn seconds(self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * DAY
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
    }

    /// The date and time `seconds` from the epoch.
    pub fn from_seconds(seconds: i64) -> Self {
        let days = seconds.div_euclid(DAY);
        let rest = seconds.rem_euclid(DAY);
        let (year, month, day) = civil_from_days(days);
        DateTime {
            year,
            month,
            day,
            hour: (rest / 3600) as u8,
            minute: (rest / 60 % 60) as u8,
            second: (rest % 60) as u8,
        }
    }
}

/// Whether `year` has a 29 February.
pub fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days in `month` of `year`.
pub fn month_days(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to `year`-`month`-`day`.
pub fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01.
pub fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year as i32, month, day)
}

/// The weekday of `days` after 1970-01-01: 0 for Sunday to 6 for Saturday.
pub fn weekday(days: i64) -> u8 {
    (days + 4).rem_euclid(7) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_days_and_weekdays() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(weekday(0), 4, "1970-01-01 was a Thursday");
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        let d = days_from_civil(2024, 3, 10);
        assert_eq!(weekday(d), 0, "2024-03-10 was a Sunday");
        assert_eq!(civil_from_days(d), (2024, 3, 10));
    }

    #[test]
    fn every_day_from_1800_to_2400_round_trips() {
        let start = days_from_civil(1800, 1, 1);
        let end = days_from_civil(2400, 1, 1);
        let (mut y, mut m, mut d) = (1800, 1, 1);
        for days in start..end {
            assert_eq!(civil_from_days(days), (y, m, d));
            assert_eq!(days_from_civil(y, m, d), days);
            d += 1;
            if d > month_days(y, m) {
                d = 1;
                m += 1;
                if m > 12 {
                    m = 1;
                    y += 1;
                }
            }
        }
    }

    #[test]
    fn seconds_round_trip_either_side_of_the_epoch() {
        let t = DateTime::date(2024, 11, 3).at(6, 30, 59);
        assert_eq!(t.seconds(), 1_730_615_459);
        assert_eq!(DateTime::from_seconds(t.seconds()), t);
        let before = DateTime::from_seconds(-1);
        assert_eq!(before, DateTime::date(1969, 12, 31).at(23, 59, 59));
        assert!(is_leap(2000) && is_leap(2024) && !is_leap(1900) && !is_leap(2023));
    }
}
