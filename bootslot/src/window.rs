//! When an update may restart the machine by itself (ADR 0008 question 3, P4
//! plan question 10): the Settings app's `/system/settings/updates.cbor`,
//! `[1, {"restart_window": [start, end] | null}]`, minutes after midnight. No
//! window, the default, means only when the user says "restart". Times are
//! UTC until the OS has a time zone.

use minicbor::Decoder;
use minicbor::data::Type;

/// Minutes in a day: a window's ends are below it.
pub const DAY_MINUTES: u16 = 24 * 60;
/// The file's schema.
const SCHEMA: u32 = 1;

/// `(start, end)` as a window, if both are minutes of a day and differ.
pub fn window(start: u16, end: u16) -> Option<(u16, u16)> {
    (start < DAY_MINUTES && end < DAY_MINUTES && start != end).then_some((start, end))
}

/// The window `updates.cbor` holds; `None` for none, or for a file that does
/// not read, so a damaged file never restarts the machine.
pub fn restart_window(bytes: &[u8]) -> Option<(u16, u16)> {
    let mut d = Decoder::new(bytes);
    if d.array().ok()? != Some(2) || d.u32().ok()? != SCHEMA {
        return None;
    }
    let entries = d.map().ok()??;
    let mut found = None;
    for _ in 0..entries {
        let key = d.str().ok()?;
        if key != "restart_window" {
            d.skip().ok()?;
            continue;
        }
        if d.datatype().ok()? == Type::Null {
            d.null().ok()?;
            found = None;
            continue;
        }
        if d.array().ok()? != Some(2) {
            return None;
        }
        found = window(d.u16().ok()?, d.u16().ok()?);
    }
    found
}

/// Whether minute `now` of the day falls in `w`; a window may run past
/// midnight.
pub fn allows(w: (u16, u16), now: u16) -> bool {
    let (start, end) = w;
    if start < end {
        (start..end).contains(&now)
    } else {
        now >= start || now < end
    }
}

/// Minute of the day, UTC, at `unix` seconds.
pub fn minute_of_day(unix: u64) -> u16 {
    ((unix / 60) % u64::from(DAY_MINUTES)) as u16
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    /// `[1, {"restart_window": [start, end]}]`, as the Settings app writes it.
    fn file(start: u16, end: u16) -> [u8; 27] {
        let mut b = [0u8; 27];
        let mut e = minicbor::Encoder::new(&mut b[..]);
        e.array(2).unwrap().u32(1).unwrap().map(1).unwrap();
        e.str("restart_window").unwrap();
        e.array(2).unwrap().u16(start).unwrap().u16(end).unwrap();
        b
    }

    #[test]
    fn the_settings_file_gives_its_window() {
        assert_eq!(restart_window(&file(120, 300)), Some((120, 300)));
        assert_eq!(restart_window(&file(1380, 60)), Some((1380, 60)));
    }

    #[test]
    fn no_window_null_or_a_bad_file_never_restarts() {
        // [1, {"restart_window": null}]
        let null = [0x82, 0x01, 0xa1, 0x6e]
            .iter()
            .copied()
            .chain(*b"restart_window")
            .chain([0xf6])
            .collect::<std::vec::Vec<u8>>();
        assert_eq!(restart_window(&null), None);
        assert_eq!(restart_window(&file(300, 300)), None);
        assert_eq!(restart_window(&file(0, DAY_MINUTES)), None);
        assert_eq!(restart_window(&[]), None);
        let mut other_schema = file(120, 300);
        other_schema[1] = 0x02;
        assert_eq!(restart_window(&other_schema), None);
    }

    #[test]
    fn a_window_holds_its_start_not_its_end_and_may_pass_midnight() {
        assert!(allows((120, 300), 120));
        assert!(allows((120, 300), 299));
        assert!(!allows((120, 300), 300));
        assert!(!allows((120, 300), 119));
        assert!(allows((1380, 60), 1439));
        assert!(allows((1380, 60), 0));
        assert!(!allows((1380, 60), 60));
        assert!(!allows((1380, 60), 720));
        assert_eq!(minute_of_day(86_400 + 125 * 60 + 59), 125);
    }
}
