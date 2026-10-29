//! The events `desktop` posts for standing instructions (P3 task 11): a window
//! minimized, restored, focused or closed, named by the app (the service) it
//! belongs to, and idle, whole minutes with no input. `desktop` hands
//! [`Watch`] the desk after each round and posts what comes back with
//! EVENT_POST; the kernel adds the time of day itself.

use crate::desk::{Focus, Window, WindowId};
use alloc::string::String;
use alloc::vec::Vec;
use kabi::event::{Event, Kind, MAX_IDLE};

const MINUTE_MS: u64 = 60_000;

struct Seen {
    id: WindowId,
    app: String,
    minimized: bool,
}

/// What the desk looked like last round, and when input last came.
pub struct Watch {
    seen: Vec<Seen>,
    focus: Focus,
    input_ms: u64,
    /// The idle minutes already posted since the last input.
    idle_sent: u16,
}

impl Watch {
    /// Starts at `now_ms`, as if input had just come.
    pub fn new(now_ms: u64) -> Self {
        Watch {
            seen: Vec::new(),
            focus: Focus::Nothing,
            input_ms: now_ms,
            idle_sent: 0,
        }
    }

    /// Input came at `now_ms`: idle starts again.
    pub fn input(&mut self, now_ms: u64) {
        self.input_ms = now_ms;
        self.idle_sent = 0;
    }

    /// `idle <n>` once for each whole minute without input, up to a day.
    pub fn idle(&mut self, now_ms: u64) -> Option<Event> {
        let minutes = now_ms.saturating_sub(self.input_ms) / MINUTE_MS;
        let minutes = u16::try_from(minutes).unwrap_or(u16::MAX).min(MAX_IDLE);
        if minutes <= self.idle_sent {
            return None;
        }
        self.idle_sent = minutes;
        Event::timed(Kind::Idle(minutes))
    }

    /// The window events since last round: minimizes and restores, closes, and
    /// a focus that moved to a window. A window's app is its process's label;
    /// one whose label no event can carry is left out.
    pub fn windows(&mut self, windows: &[Window], focus: Focus) -> Vec<Event> {
        let mut out = Vec::new();
        let mut post = |kind, app: &str| out.extend(Event::window(kind, app));
        for window in windows {
            match self.seen.iter_mut().find(|s| s.id == window.id) {
                None => self.seen.push(Seen {
                    id: window.id,
                    app: window.label.clone(),
                    minimized: window.minimized,
                }),
                Some(seen) if seen.minimized != window.minimized => {
                    seen.minimized = window.minimized;
                    let kind = match window.minimized {
                        true => Kind::Minimize,
                        false => Kind::Restore,
                    };
                    post(kind, &seen.app);
                }
                Some(_) => {}
            }
        }
        self.seen.retain(|seen| {
            let open = windows.iter().any(|w| w.id == seen.id);
            if !open {
                post(Kind::Close, &seen.app);
            }
            open
        });
        if focus != self.focus {
            self.focus = focus;
            if let Focus::Window(id) = focus
                && let Some(seen) = self.seen.iter().find(|s| s.id == id)
            {
                post(Kind::Focus, &seen.app);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Rect, Size};
    use alloc::format;

    fn window(id: u32, label: &str, minimized: bool) -> Window {
        Window {
            id: WindowId(id),
            client: 1,
            label: label.into(),
            title: label.into(),
            app_id: String::new(),
            frame: Rect::new(0, 0, 10, 10),
            minimized,
            restore: None,
            min: Size { w: 1, h: 1 },
            max: Size { w: 100, h: 100 },
        }
    }

    fn lines(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                let mut text = String::new();
                e.text(&mut text).unwrap();
                text
            })
            .collect()
    }

    #[test]
    fn window_changes_become_events_once() {
        let mut watch = Watch::new(0);
        let notes = window(1, "notes", false);
        assert!(
            watch
                .windows(core::slice::from_ref(&notes), Focus::Nothing)
                .is_empty()
        );
        let focused = Focus::Window(WindowId(1));
        assert_eq!(
            lines(&watch.windows(core::slice::from_ref(&notes), focused)),
            ["focus notes"]
        );
        assert!(
            watch
                .windows(core::slice::from_ref(&notes), focused)
                .is_empty()
        );
        let down = window(1, "notes", true);
        assert_eq!(
            lines(&watch.windows(core::slice::from_ref(&down), Focus::Nothing)),
            ["minimize notes"]
        );
        assert!(watch.windows(&[down], Focus::Nothing).is_empty());
        assert_eq!(
            lines(&watch.windows(core::slice::from_ref(&notes), focused)),
            ["restore notes", "focus notes"]
        );
        assert_eq!(lines(&watch.windows(&[], Focus::Nothing)), ["close notes"]);
        assert!(watch.windows(&[], Focus::Nothing).is_empty());
    }

    #[test]
    fn a_label_no_event_can_carry_is_left_out() {
        let mut watch = Watch::new(0);
        let odd = window(2, "pid 7", false);
        watch.windows(core::slice::from_ref(&odd), Focus::Nothing);
        let mut down = odd.clone();
        down.minimized = true;
        assert!(
            watch
                .windows(&[down], Focus::Window(WindowId(2)))
                .is_empty()
        );
    }

    #[test]
    fn idle_counts_whole_minutes_once_each() {
        let mut watch = Watch::new(1_000);
        assert_eq!(watch.idle(1_000 + MINUTE_MS - 1), None);
        assert_eq!(lines(&[watch.idle(1_000 + MINUTE_MS).unwrap()]), ["idle 1"]);
        assert_eq!(watch.idle(1_000 + MINUTE_MS + 5), None);
        // A late round names the minutes reached, not each one missed.
        assert_eq!(
            lines(&[watch.idle(1_000 + 3 * MINUTE_MS).unwrap()]),
            ["idle 3"]
        );
        watch.input(1_000 + 3 * MINUTE_MS + 1);
        assert_eq!(watch.idle(1_000 + 4 * MINUTE_MS), None);
        let day = 1_000 + 3 * MINUTE_MS + 1 + u64::from(MAX_IDLE + 5) * MINUTE_MS;
        assert_eq!(
            lines(&[watch.idle(day).unwrap()]),
            [format!("idle {MAX_IDLE}")]
        );
        assert_eq!(watch.idle(day + MINUTE_MS), None);
    }
}
