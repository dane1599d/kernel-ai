//! What `desktop` and the kernel tell the shell's standing instructions (P3
//! task 11): window events, idle and the time of day, through the kernel's event box
//! (EVENT_POST, EVENT_READ). One event is one line of text, `<kind> <arg>`:
//!
//! - `minimize notes`, `restore notes`, `focus notes`, `close notes`: a window of
//!   the app (the service) `notes`;
//! - `idle 5`: no input for 5 whole minutes;
//! - `time 545`: minute 545 of the day (09:05 UTC) began; the kernel adds these.
//!
//! The kernel checks each line with [`Event::parse`] before it keeps it, and
//! EVENT_READ hands lines back as [`Event::text`] writes them, so a reader can
//! trust the shape, never where the event came from beyond "the display holder
//! or the kernel".

use core::fmt::Write;

/// The longest app name an event carries.
pub const MAX_NAME: usize = 32;
/// The longest event line.
pub const MAX_LINE: usize = 8 + MAX_NAME;
/// Events the box keeps; past that the oldest goes, and is counted.
pub const BOX: usize = 32;
/// The buffer EVENT_READ needs: a full box and its `dropped` line.
pub const READ_BYTES: usize = (BOX + 1) * (MAX_LINE + 1);
/// Minutes in a day: `time` runs 0 to `DAY_MINUTES - 1`.
pub const DAY_MINUTES: u16 = 1440;
/// The longest idle an event names, in minutes (a day).
pub const MAX_IDLE: u16 = DAY_MINUTES;

/// What happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Minimize,
    Restore,
    Focus,
    Close,
    /// No input for this many minutes.
    Idle(u16),
    /// This minute of the day began.
    Time(u16),
}

/// One event: a kind and, for a window event, the app it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: Kind,
    name: [u8; MAX_NAME],
    len: u8,
}

/// An app name an event may carry: a service name's characters.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

impl Event {
    /// A window event for app `name`; `None` for a name [`valid_name`] refuses.
    pub fn window(kind: Kind, name: &str) -> Option<Self> {
        if matches!(kind, Kind::Idle(_) | Kind::Time(_)) || !valid_name(name) {
            return None;
        }
        let mut event = Event {
            kind,
            name: [0; MAX_NAME],
            len: name.len() as u8,
        };
        event.name[..name.len()].copy_from_slice(name.as_bytes());
        Some(event)
    }

    /// `idle` or `time`, which name no app; `None` past their range.
    pub fn timed(kind: Kind) -> Option<Self> {
        let ok = match kind {
            Kind::Idle(minutes) => (1..=MAX_IDLE).contains(&minutes),
            Kind::Time(minute) => minute < DAY_MINUTES,
            _ => false,
        };
        ok.then_some(Event {
            kind,
            name: [0; MAX_NAME],
            len: 0,
        })
    }

    /// The app a window event names; "" for idle and time.
    pub fn app(&self) -> &str {
        core::str::from_utf8(&self.name[..usize::from(self.len)]).unwrap_or("")
    }

    /// One line, strictly: anything else is `None`.
    pub fn parse(line: &str) -> Option<Self> {
        let (word, arg) = line.split_once(' ')?;
        let number = || -> Option<u16> {
            (!arg.starts_with('0') || arg == "0")
                .then(|| arg.parse().ok())
                .flatten()
        };
        match word {
            "minimize" => Event::window(Kind::Minimize, arg),
            "restore" => Event::window(Kind::Restore, arg),
            "focus" => Event::window(Kind::Focus, arg),
            "close" => Event::window(Kind::Close, arg),
            "idle" => Event::timed(Kind::Idle(number()?)),
            "time" => Event::timed(Kind::Time(number()?)),
            _ => None,
        }
    }

    /// The line [`Event::parse`] reads.
    pub fn text(&self, out: &mut impl Write) -> core::fmt::Result {
        match self.kind {
            Kind::Minimize => write!(out, "minimize {}", self.app()),
            Kind::Restore => write!(out, "restore {}", self.app()),
            Kind::Focus => write!(out, "focus {}", self.app()),
            Kind::Close => write!(out, "close {}", self.app()),
            Kind::Idle(minutes) => write!(out, "idle {minutes}"),
            Kind::Time(minute) => write!(out, "time {minute}"),
        }
    }
}

/// EVENT_READ's first line when events were dropped since the last read:
/// `dropped <n>`.
pub const DROPPED: &str = "dropped ";

/// Actions a standing instruction may run, each with the inverse its "until"
/// and `undo` run. The kernel notes no other, so no other fires.
pub const STANDING: &[(&str, &str)] = &[("app_suspend", "app_resume")];

/// The action and app of the standing instruction `call` makes: `app_suspend`
/// and `notes` of `rules_add("on minimize(notes) -> app_suspend(notes) until
/// restore(notes)")`; `None` for anything else, or an action [`STANDING`] does
/// not list.
pub fn standing(call: &str) -> Option<(&str, &str)> {
    let rule = call.strip_prefix("rules_add(\"")?.strip_suffix("\")")?;
    let (_, then) = rule.split_once(" -> ")?;
    let then = then.split_once(" until ").map_or(then, |(then, _)| then);
    let (action, target) = then.strip_suffix(')')?.split_once('(')?;
    let listed = STANDING.iter().any(|(a, _)| *a == action);
    (listed && valid_name(target)).then_some((action, target))
}

/// Whether the standing instruction for `action` on `target` covers `call`:
/// exactly `action("target")`, or its inverse on the same app.
pub fn covers(action: &str, target: &str, call: &str) -> bool {
    let Some((_, inverse)) = STANDING.iter().find(|(a, _)| *a == action) else {
        return false;
    };
    [action, inverse].iter().any(|a| {
        call.strip_prefix(a)
            .and_then(|rest| rest.strip_prefix("(\""))
            .and_then(|rest| rest.strip_suffix("\")"))
            == Some(target)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn round(line: &str) {
        let event = Event::parse(line).unwrap();
        let mut text = String::new();
        event.text(&mut text).unwrap();
        assert_eq!(text, line);
    }

    #[test]
    fn lines_round_trip() {
        for line in [
            "minimize notes",
            "restore notes",
            "focus notes-window",
            "close a_b",
            "idle 1",
            "idle 1440",
            "time 0",
            "time 1439",
        ] {
            round(line);
        }
        assert!(Event::parse("minimize notes").unwrap().app() == "notes");
    }

    #[test]
    fn anything_else_is_refused() {
        for line in [
            "",
            "minimize",
            "minimize ",
            "minimize Notes",
            "minimize notes x",
            "minimize ../etc",
            "idle 0",
            "idle 1441",
            "idle 05",
            "idle -1",
            "idle notes",
            "time 1440",
            "time 01",
            "open notes",
            "minimize  notes",
        ] {
            assert_eq!(Event::parse(line), None, "{line:?}");
        }
        assert_eq!(
            Event::parse(&alloc::format!("close {}", "a".repeat(MAX_NAME)))
                .unwrap()
                .app()
                .len(),
            MAX_NAME
        );
        assert_eq!(
            Event::parse(&alloc::format!("close {}", "a".repeat(MAX_NAME + 1))),
            None
        );
        assert_eq!(Event::window(Kind::Idle(1), "notes"), None);
        assert_eq!(Event::timed(Kind::Minimize), None);
    }

    #[test]
    fn a_standing_instruction_covers_its_action_and_inverse_only() {
        let made = "rules_add(\"on minimize(notes) -> app_suspend(notes) until restore(notes)\")";
        let (action, target) = standing(made).unwrap();
        assert_eq!((action, target), ("app_suspend", "notes"));
        assert!(covers(action, target, "app_suspend(\"notes\")"));
        assert!(covers(action, target, "app_resume(\"notes\")"));
        assert!(!covers(action, target, "app_suspend(\"files\")"));
        assert!(!covers(action, target, "service_stop(\"notes\")"));
        assert!(!covers(action, target, "app_suspend(\"notes\") "));
        assert!(!covers("app_resume", target, "app_resume(\"notes\")"));
    }

    #[test]
    fn only_listed_actions_on_app_names_are_noted() {
        for call in [
            "rules_add(\"on focus(game) -> profile_set(gaming)\")",
            "rules_add(\"on idle(5) -> app_suspend(Notes)\")",
            "rules_add(\"on idle(5) -> app_suspend(../x)\")",
            "rules_add(\"on idle(5) -> app_suspend()\")",
            "service_stop(\"notes\")",
            "rules_add(\"app_suspend(notes)\")",
        ] {
            assert_eq!(standing(call), None, "{call}");
        }
        assert_eq!(
            standing("rules_add(\"on idle(5) -> app_suspend(notes)\")"),
            Some(("app_suspend", "notes"))
        );
    }
}
