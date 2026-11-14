//! "What time is it?" answered from the kernel's clock, never by the model
//! (founder test 2, item 12): asked before, the model gave an invented time or
//! an unrelated change card. The act pack is at its token bar, so this is a
//! fixed answer in code rather than a new row.

use alloc::format;
use alloc::string::String;

/// Lines that ask for the time or the date, after lowercasing and without a
/// trailing `?`, `.` or `!`.
const ASKS: &[&str] = &[
    "what time is it",
    "what time is it now",
    "what's the time",
    "whats the time",
    "what is the time",
    "time",
    "the time",
    "what's the date",
    "whats the date",
    "what is the date",
    "what's today's date",
    "what is today's date",
    "what date is it",
    "what day is it",
    "what day is it today",
    "what is today",
    "date",
];

const DAYS: [&str; 7] = [
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
];

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const SECONDS_PER_DAY: u64 = 86_400;

/// The answer to `line` if it asks for the time or the date, given the clock's
/// seconds since 1970 UTC (`None` when the firmware gave no date).
pub(crate) fn answer(line: &str, now: Option<u64>) -> Option<String> {
    let asked = line
        .trim()
        .trim_end_matches(['?', '.', '!'])
        .trim()
        .to_ascii_lowercase();
    if !ASKS.contains(&asked.as_str()) {
        return None;
    }
    Some(match now {
        Some(seconds) => format!("It is {}.", describe(seconds)),
        None => String::from("I can't tell the time: this machine gave no date when it started."),
    })
}

/// `14:05 UTC on Saturday 14 November 2026`.
pub(crate) fn describe(seconds: u64) -> String {
    let days = seconds / SECONDS_PER_DAY;
    let of_day = seconds % SECONDS_PER_DAY;
    let (year, month, day) = civil(days);
    // 1970-01-01 was a Thursday.
    let weekday = DAYS[(days % 7) as usize];
    format!(
        "{:02}:{:02} UTC on {weekday} {day} {} {year}",
        of_day / 3600,
        of_day % 3600 / 60,
        MONTHS[(month - 1) as usize]
    )
}

/// The proleptic Gregorian date of day `days` since 1970-01-01, as (year,
/// month 1-12, day 1-31): Howard Hinnant's `civil_from_days`.
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
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

    #[test]
    fn the_date_and_time_read_as_a_person_says_them() {
        for (seconds, said) in [
            (0, "00:00 UTC on Thursday 1 January 1970"),
            (1_794_637_800, "06:30 UTC on Saturday 14 November 2026"),
            (951_868_740, "23:59 UTC on Tuesday 29 February 2000"),
            (1_735_646_400, "12:00 UTC on Tuesday 31 December 2024"),
        ] {
            assert_eq!(describe(seconds), said);
        }
    }

    #[test]
    fn only_a_question_for_the_time_or_date_is_answered() {
        assert_eq!(
            answer("What time is it?", Some(1_794_637_800)).as_deref(),
            Some("It is 06:30 UTC on Saturday 14 November 2026.")
        );
        assert!(answer("what's the date", None).is_some_and(|text| text.contains("no date")));
        for line in [
            "what time is the meeting",
            "set the time",
            "show me the cpus",
        ] {
            assert_eq!(answer(line, Some(0)), None, "{line}");
        }
    }
}
