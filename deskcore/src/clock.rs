//! The tray's clock (ADR 0005 §8): seconds since 1970 UTC, as the kernel's
//! wall clock gives them, as "14:05" and "14 Nov UTC", or in the person's
//! fixed offset (`look::appearance`'s `time_zone`, no daylight saving) as
//! "16:05" and "14 Nov UTC+2". Pure, so midnight, month ends, leap years and
//! a day crossed by the offset are tested without a clock. With no wall clock
//! (`CLOCK_NOW` said NotFound) there is no clock to draw.

use alloc::format;
use alloc::string::String;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The seconds on the wall in a zone `zone_min` minutes from UTC (before
/// 1970 there is nothing to show: 0).
fn local(utc_s: u64, zone_min: i16) -> u64 {
    utc_s.saturating_add_signed(i64::from(zone_min) * 60)
}

/// The zone as the date line names it: "UTC", "UTC+2", "UTC-9:30".
pub fn label(zone_min: i16) -> String {
    let (sign, m) = (if zone_min < 0 { '-' } else { '+' }, zone_min.abs());
    match (zone_min, m % 60) {
        (0, _) => String::from("UTC"),
        (_, 0) => format!("UTC{sign}{}", m / 60),
        _ => format!("UTC{sign}{}:{:02}", m / 60, m % 60),
    }
}

/// The time of day in UTC, "HH:MM".
pub fn time(utc_s: u64) -> String {
    time_in(utc_s, 0)
}

/// The time of day in the zone, "HH:MM".
pub fn time_in(utc_s: u64, zone_min: i16) -> String {
    let day_s = local(utc_s, zone_min) % 86_400;
    format!("{:02}:{:02}", day_s / 3_600, day_s % 3_600 / 60)
}

/// The date in UTC, "D Mon UTC".
pub fn date(utc_s: u64) -> String {
    date_in(utc_s, 0)
}

/// The date in the zone, "D Mon UTC+2".
pub fn date_in(utc_s: u64, zone_min: i16) -> String {
    let (_, month, day) = civil(local(utc_s, zone_min) / 86_400);
    format!("{day} {} {}", MONTHS[(month - 1) as usize], label(zone_min))
}

/// The minute `utc_s` falls in: what the clock shows changes only with it.
pub fn minute(utc_s: u64) -> u64 {
    minute_in(utc_s, 0)
}

/// The minute on the wall in the zone: it changes with the time, and with
/// the zone, since an offset is never a whole day.
pub fn minute_in(utc_s: u64, zone_min: i16) -> u64 {
    local(utc_s, zone_min) / 60
}

/// Days since 1970-01-01 as (year, month 1-12, day 1-31), by Howard
/// Hinnant's civil-from-days.
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seconds at `y-m-d hh:mm` UTC, days counted the slow way.
    fn at(y: u64, m: u64, d: u64, hh: u64, mm: u64) -> u64 {
        let leap =
            |y: u64| (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
        let mut days = (1970..y)
            .map(|y| if leap(y) { 366 } else { 365 })
            .sum::<u64>();
        let lengths = [
            31,
            if leap(y) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        days += lengths[..(m - 1) as usize].iter().sum::<u64>() + d - 1;
        days * 86_400 + hh * 3_600 + mm * 60
    }

    #[test]
    fn the_clock_reads_the_time_and_the_date() {
        assert_eq!(time(0), "00:00");
        assert_eq!(date(0), "1 Jan UTC");
        let s = at(2026, 11, 14, 14, 5);
        assert_eq!((time(s), date(s)), ("14:05".into(), "14 Nov UTC".into()));
        assert_eq!(time(s + 59), "14:05");
        assert_eq!(minute(s + 59), minute(s));
        assert_eq!(minute(s + 60), minute(s) + 1);
    }

    #[test]
    fn midnight_rolls_the_date_over() {
        let before = at(2026, 12, 31, 23, 59) + 59;
        assert_eq!(
            (time(before), date(before)),
            ("23:59".into(), "31 Dec UTC".into())
        );
        let after = before + 1;
        assert_eq!(
            (time(after), date(after)),
            ("00:00".into(), "1 Jan UTC".into())
        );
    }

    #[test]
    fn a_fixed_offset_moves_the_time_and_can_cross_the_date() {
        let s = at(2026, 12, 31, 23, 30);
        assert_eq!(
            (time_in(s, 0), date_in(s, 0)),
            ("23:30".into(), "31 Dec UTC".into())
        );
        // East of UTC the year turns first; west of it, later.
        assert_eq!(
            (time_in(s, 120), date_in(s, 120)),
            ("01:30".into(), "1 Jan UTC+2".into())
        );
        assert_eq!(
            (time_in(s, 345), date_in(s, 345)),
            ("05:15".into(), "1 Jan UTC+5:45".into())
        );
        assert_eq!(
            (time_in(s, -570), date_in(s, -570)),
            ("14:00".into(), "31 Dec UTC-9:30".into())
        );
        let early = at(2026, 3, 1, 1, 0);
        assert_eq!(date_in(early, -300), "28 Feb UTC-5");
        assert_eq!(date_in(at(2028, 2, 28, 23, 0), 60), "29 Feb UTC+1");
        // The same instant, an hour on either side of the zone's midnight.
        let midnight = at(2026, 6, 10, 22, 0);
        assert_eq!(date_in(midnight - 1, 120), "10 Jun UTC+2");
        assert_eq!(date_in(midnight, 120), "11 Jun UTC+2");
        assert_eq!(time_in(midnight, 120), "00:00");
    }

    #[test]
    fn the_minute_follows_the_zone_and_the_utc_calls_are_zone_zero() {
        let s = at(2026, 11, 14, 14, 5);
        assert_eq!(minute_in(s, 0), minute(s));
        assert_ne!(minute_in(s, 60), minute_in(s, 0));
        assert_eq!(minute_in(s + 59, 90), minute_in(s, 90));
        assert_eq!((time(s), date(s)), (time_in(s, 0), date_in(s, 0)));
        // Never before 1970, whatever the zone.
        assert_eq!(time_in(0, -720), "00:00");
        assert_eq!(date_in(0, -720), "1 Jan UTC-12");
    }

    #[test]
    fn leap_years_have_their_29th_of_february() {
        for (y, leap) in [(2024, true), (2025, false), (2000, true), (2100, false)] {
            let feb28 = at(y, 2, 28, 12, 0);
            let next = date(feb28 + 86_400);
            assert_eq!(next == "29 Feb UTC", leap, "{y}: {next}");
        }
    }
}
