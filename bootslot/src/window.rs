//! When an update may restart the machine by itself (ADR 0008 question 3, P4
//! plan question 10): the window the shell saves from a Settings click, in
//! `/system/state/restart/window`, as `kai-updates 1` text:
//!
//! ```text
//! kai-updates 1
//! restart-window <start> <end>
//! ```
//!
//! in minutes after midnight, the second line only while a window is set. No
//! window, the default, means only when the user says "restart". Times are UTC
//! until the OS has a time zone.

/// Minutes in a day: a window's ends are below it.
pub const DAY_MINUTES: u16 = 24 * 60;
/// The file's first line.
pub const HEADER: &str = "kai-updates 1";
/// The window's line, before its two minutes.
pub const LINE: &str = "restart-window";
/// The file's path beneath `/system/state`; `updated`'s `dir=restart` gives it
/// the folder alone.
pub const FOLDER: &str = "restart";
pub const FILE: &str = "window";

/// `(start, end)` as a window, if both are minutes of a day and differ.
pub fn window(start: u16, end: u16) -> Option<(u16, u16)> {
    (start < DAY_MINUTES && end < DAY_MINUTES && start != end).then_some((start, end))
}

/// The window `kai-updates 1` text holds; `None` for none, or for text out of
/// shape, so a damaged file never restarts the machine.
pub fn restart_window(text: &str) -> Option<(u16, u16)> {
    let mut lines = text.lines();
    if lines.next()? != HEADER {
        return None;
    }
    let line = lines.next()?;
    if lines.next().is_some() {
        return None;
    }
    let mut words = line.split(' ');
    if words.next()? != LINE {
        return None;
    }
    let minute = |word: Option<&str>| word?.parse::<u16>().ok();
    let (start, end) = (minute(words.next())?, minute(words.next())?);
    if words.next().is_some() {
        return None;
    }
    window(start, end)
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
    use super::*;

    #[test]
    fn the_saved_text_gives_its_window() {
        assert_eq!(
            restart_window("kai-updates 1\nrestart-window 120 300\n"),
            Some((120, 300))
        );
        assert_eq!(
            restart_window("kai-updates 1\nrestart-window 1380 60"),
            Some((1380, 60))
        );
    }

    #[test]
    fn no_window_or_text_out_of_shape_never_restarts() {
        for text in [
            "",
            "kai-updates 1\n",
            "kai-updates 2\nrestart-window 120 300\n",
            "kai-updates 1\nrestart-window 300 300\n",
            "kai-updates 1\nrestart-window 0 1440\n",
            "kai-updates 1\nrestart-window 120\n",
            "kai-updates 1\nrestart-window 120 300 9\n",
            "kai-updates 1\nrestart-window -1 300\n",
            "kai-updates 1\nrestart-window 120  300\n",
            "kai-updates 1\nrestart-window 120 300\nrestart-window 1 2\n",
            "kai-updates 1\nrestart 120 300\n",
        ] {
            assert_eq!(restart_window(text), None, "{text:?}");
        }
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
