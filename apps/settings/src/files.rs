//! What the Settings pages read of the system's settings. The shell keeps
//! them and answers `file appearance`, `file apps` and `file updates` with them
//! as text (A-core's `kai-appearance 1`, `kai-keep 1` and `kai-updates 1`). An
//! empty answer, or one out of shape, gives the defaults.

use alloc::string::String;
use alloc::vec::Vec;

/// The shell's saved appearance (`kai-appearance 1`, `look::appearance`): the
/// theme by `look` palette name, and the two switches ADR 0005 §8 names. The
/// other settings in the file (scale, text size, colour tokens) are the
/// shell's and `desktop`'s, and are not shown here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub theme: String,
    pub legible: bool,
    pub reduced_motion: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: String::from(look::THEMES[0].name),
            legible: false,
            reduced_motion: false,
        }
    }
}

/// The shell's keep-running list (`kai-keep 1`): the apps the user keeps
/// running, which app nap never suspends (founder, 2026-10-28; P3 task 12b).
/// Sorted, each once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Apps {
    pub keep_running: Vec<String>,
}

/// The shell's restart window (`kai-updates 1`): when an update may restart the
/// machine by itself (ADR 0008 question 3, P4 plan question 10). `None`, the
/// default, means only when the user says "restart"; otherwise minutes after
/// midnight, start and end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Updates {
    pub restart_window: Option<(u16, u16)>,
}

/// Minutes in a day: a window's ends are below it.
pub const DAY_MINUTES: u16 = 24 * 60;

impl Appearance {
    /// The shell's `kai-appearance 1` text over the defaults. Text that is not
    /// that, a setting `look` refuses, or an empty answer (nothing saved) gives
    /// the defaults, as does a theme `look` does not ship.
    pub fn decode(bytes: &[u8]) -> Self {
        let Ok(text) = core::str::from_utf8(bytes) else {
            return Appearance::default();
        };
        match look::appearance::Appearance::decode(text, &look::appearance::Shipped) {
            Ok(saved) => Appearance {
                theme: saved.theme,
                legible: saved.legible,
                reduced_motion: saved.reduced_motion,
            },
            Err(_) => Appearance::default(),
        }
    }
}

/// The first line of the shell's keep-running list.
pub const KEEP_HEADER: &str = "kai-keep 1";
/// Most apps the keep-running list holds.
pub const MAX_KEPT: usize = 256;

impl Apps {
    /// The shell's `kai-keep 1` text: the header, then one app a line. Anything
    /// else, or an empty answer (none kept), is no app; a line that is not an
    /// app name is left out.
    pub fn decode(bytes: &[u8]) -> Self {
        let mut apps = Apps::default();
        let Ok(text) = core::str::from_utf8(bytes) else {
            return apps;
        };
        let mut lines = text.lines();
        if lines.next() != Some(KEEP_HEADER) {
            return apps;
        }
        for name in lines.filter(|name| crate::access::app_ok(name)) {
            apps.set(name, true);
        }
        apps
    }

    pub fn keeps(&self, app: &str) -> bool {
        self.keep_running.iter().any(|a| a == app)
    }

    /// Turns `app`'s keep-running switch on or off; false if nothing changed.
    pub fn set(&mut self, app: &str, on: bool) -> bool {
        match (
            self.keep_running.binary_search_by(|a| a.as_str().cmp(app)),
            on,
        ) {
            (Ok(_), true) | (Err(_), false) => false,
            (Err(_), true) if self.keep_running.len() >= MAX_KEPT => false,
            (Err(at), true) => {
                self.keep_running.insert(at, String::from(app));
                true
            }
            (Ok(at), false) => {
                self.keep_running.remove(at);
                true
            }
        }
    }
}

/// The first line of the shell's restart-window text.
pub const UPDATES_HEADER: &str = "kai-updates 1";

/// A minute of the day as digits alone, so a sign or a space is not one.
fn minute(word: &str) -> Option<u16> {
    let digits = !word.is_empty() && word.len() <= 4 && word.bytes().all(|b| b.is_ascii_digit());
    digits.then(|| word.parse().ok()).flatten()
}

impl Updates {
    /// The shell's `kai-updates 1` text: the header, then `restart-window
    /// <start> <end>` while one is set. An empty answer (none), anything out of
    /// shape, or a window whose ends are not two different minutes of a day is
    /// no window.
    pub fn decode(bytes: &[u8]) -> Self {
        let mut updates = Updates::default();
        let Ok(text) = core::str::from_utf8(bytes) else {
            return updates;
        };
        let mut lines = text.lines();
        if lines.next() != Some(UPDATES_HEADER) {
            return updates;
        }
        for line in lines {
            let mut words = line.split(' ');
            if let (Some("restart-window"), Some(start), Some(end), None) =
                (words.next(), words.next(), words.next(), words.next())
                && let (Some(start), Some(end)) = (minute(start), minute(end))
            {
                updates.restart_window = window(start, end);
            }
        }
        updates
    }

    /// Whether minute `now` of the day falls in the window; a window may run
    /// past midnight.
    pub fn allows(&self, now: u16) -> bool {
        match self.restart_window {
            None => false,
            Some((start, end)) if start < end => (start..end).contains(&now),
            Some((start, end)) => now >= start || now < end,
        }
    }
}

/// `(start, end)` as a window, if both are minutes of a day and differ.
pub fn window(start: u16, end: u16) -> Option<(u16, u16)> {
    (start < DAY_MINUTES && end < DAY_MINUTES && start != end).then_some((start, end))
}
