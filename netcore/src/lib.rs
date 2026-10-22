//! The network setup's logic without a network card, tested on the host: the
//! command line's address settings (`config`), DNS queries and replies (`dns`),
//! the resolver's TTL cache (`cache`), ping statistics (`ping`), and the test
//! DNS zone (`fixture`). The kernel
//! owns the card, the smoltcp interface and the clock, and drives these.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod cache;
pub mod config;
pub mod dns;
pub mod fixture;
pub mod ping;

use alloc::format;
use alloc::string::String;

/// A span of whole seconds as `1d 2h 3m 4s`, leaving out the zero parts.
pub fn duration(seconds: u64) -> String {
    if seconds == 0 {
        return String::from("0s");
    }
    let parts = [
        (seconds / 86_400, "d"),
        (seconds / 3600 % 24, "h"),
        (seconds / 60 % 60, "m"),
        (seconds % 60, "s"),
    ];
    let mut text = String::new();
    for (value, unit) in parts.into_iter().filter(|&(value, _)| value > 0) {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!("{value}{unit}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::duration;

    #[test]
    fn durations_leave_out_zero_parts() {
        assert_eq!(duration(0), "0s");
        assert_eq!(duration(59), "59s");
        assert_eq!(duration(3600), "1h");
        assert_eq!(duration(86_400), "1d");
        assert_eq!(duration(90_061), "1d 1h 1m 1s");
    }
}
