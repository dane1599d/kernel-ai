//! A time a request names, resolved by code from the clock, never by the
//! model (ADR 0013 section 4): the earliest change time a result may have, in
//! nanoseconds since the Unix epoch. Days are UTC days until the OS has a time
//! zone; a week is the last 7 days and a month the last 30, not the calendar's.

use alloc::string::String;
use alloc::vec::Vec;

const SECOND_NS: u64 = 1_000_000_000;
pub const DAY_NS: u64 = 24 * 60 * 60 * SECOND_NS;
/// The most days "last N days" may name.
pub const MAX_DAYS: u64 = 3650;

/// The start of the time `words` names, at clock time `now_ns`, or `None` for
/// words that are not one of: `today`, `yesterday`, `last week`, `past week`,
/// `last month`, `past month`, `last N days`, `past N days` (or `day` for 1),
/// in any case and spacing. A time before the epoch is the epoch.
pub fn resolve(words: &str, now_ns: u64) -> Option<u64> {
    let lower = String::from(words).to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let today = now_ns - now_ns % DAY_NS;
    let back = |days: u64| Some(now_ns.saturating_sub(days * DAY_NS));
    match words.as_slice() {
        ["today"] => Some(today),
        ["yesterday"] => Some(today.saturating_sub(DAY_NS)),
        ["last" | "past", "week"] => back(7),
        ["last" | "past", "month"] => back(30),
        ["last" | "past", "day"] => back(1),
        ["last" | "past", n, "days" | "day"] => {
            let n: u64 = n.parse().ok().filter(|n| (1..=MAX_DAYS).contains(n))?;
            back(n)
        }
        _ => None,
    }
}
