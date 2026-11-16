//! The sends the shell holds before they go (`shellcore::outbox`, ADR 0002
//! `Hold`), as the Kai pane shows them: "Sending to hooks.example in 0:04"
//! with an `undo` that names the send's log entry, then "Sending to
//! hooks.example…" once the shell says it is going, then code's line for
//! what it came to.
//!
//! They are kept apart from the panel's one state, so a line typed during
//! the hold does not drop them: the status row shows the newest held send
//! whenever the shell is not working on a line.
//!
//! The shell reports them to `desktop` as:
//!
//! - `panel sends -` or `panel sends <entry> held|sending <left_ms> <host>;…`
//!   → `ok`: every send waiting or going, oldest first;
//! - `panel sent <entry> ok|failed <line>` → `ok`: what one came to.

use crate::panel::{Panel, State};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Sends the panel keeps at most, as the shell's outbox holds.
pub const MAX_SENDS: usize = 8;
/// The longest host a send names (a DNS name's limit).
const MAX_HOST: usize = 253;
const SENDS: &str = "panel sends ";
const SENT: &str = "panel sent ";
/// A countdown that moved less than this is no change worth a repaint.
const DRIFT_MS: u64 = 1_000;

/// One send waiting or going, as the panel keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldSend {
    /// The log entry `undo` takes back.
    pub entry: u64,
    pub host: String,
    /// When it is due, on `desktop`'s clock.
    pub due_ms: u64,
    /// The shell said it is going: no undo any more.
    pub sending: bool,
}

/// One send as the shell reports it: how long until due, not when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireSend {
    pub entry: u64,
    pub sending: bool,
    pub left_ms: u64,
    pub host: String,
}

impl WireSend {
    /// The panel's send, due `left_ms` after `now_ms`.
    pub fn held(&self, now_ms: u64) -> HeldSend {
        HeldSend {
            entry: self.entry,
            host: self.host.clone(),
            due_ms: now_ms.saturating_add(self.left_ms),
            sending: self.sending,
        }
    }
}

/// The sends waiting or going, as a request.
pub fn encode_sends(sends: &[WireSend]) -> String {
    if sends.is_empty() {
        return format!("{SENDS}-");
    }
    let each: Vec<String> = sends
        .iter()
        .take(MAX_SENDS)
        .map(|s| {
            let stage = if s.sending { "sending" } else { "held" };
            format!("{} {stage} {} {}", s.entry, s.left_ms, s.host)
        })
        .collect();
    format!("{SENDS}{}", each.join(";"))
}

/// A `panel sends` request read back; `None` for anything else, or for a
/// list that is too long or names a host that is not one.
pub fn parse_sends(text: &str) -> Option<Vec<WireSend>> {
    let body = text.strip_prefix(SENDS)?;
    if body == "-" {
        return Some(Vec::new());
    }
    let sends: Vec<WireSend> = body
        .split(';')
        .map(|one| {
            let mut words = one.split(' ');
            let entry = words.next()?.parse().ok()?;
            let sending = match words.next()? {
                "held" => false,
                "sending" => true,
                _ => return None,
            };
            let left_ms = words.next()?.parse().ok()?;
            let host = words.next().filter(|h| is_host(h))?.to_string();
            words.next().is_none().then_some(WireSend {
                entry,
                sending,
                left_ms,
                host,
            })
        })
        .collect::<Option<_>>()?;
    (sends.len() <= MAX_SENDS).then_some(sends)
}

/// What send `entry` came to, as a request: `failed` when it did not go, or
/// may have, so the panel shows it under its error bar.
pub fn encode_sent(entry: u64, failed: bool, line: &str) -> String {
    let word = if failed { "failed" } else { "ok" };
    let line = line.lines().next().unwrap_or("-").replace('\t', " ");
    let mut end = line.len().min(crate::panel_wire::MAX_TEXT);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{SENT}{entry} {word} {}", &line[..end])
}

/// A `panel sent` request read back: entry, whether it failed, the line.
pub fn parse_sent(text: &str) -> Option<(u64, bool, String)> {
    let mut parts = text.strip_prefix(SENT)?.splitn(3, ' ');
    let entry = parts.next()?.parse().ok()?;
    let failed = match parts.next()? {
        "ok" => false,
        "failed" => true,
        _ => return None,
    };
    let line = parts
        .next()
        .filter(|l| !l.is_empty() && !l.contains('\n') && l.len() <= crate::panel_wire::MAX_TEXT)?;
    Some((entry, failed, line.to_string()))
}

/// A host name or address as a send may name one: printable, no spaces.
fn is_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
}

/// How long until `due_ms`, at `now_ms`, as `m:ss`, rounded up: a send due
/// in 4.2 s says 0:05, and 0:00 only once it is due.
pub fn left(due_ms: u64, now_ms: u64) -> String {
    let seconds = due_ms.saturating_sub(now_ms).div_ceil(1000);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl Panel {
    /// The shell's list of sends waiting or going; true when what the panel
    /// shows changed (a countdown that drifted under a second is not).
    pub fn set_sends(&mut self, mut sends: Vec<HeldSend>) -> bool {
        sends.truncate(MAX_SENDS);
        let same = self.sends.len() == sends.len()
            && self.sends.iter().zip(&sends).all(|(was, now)| {
                was.entry == now.entry
                    && was.host == now.host
                    && was.sending == now.sending
                    && was.due_ms.abs_diff(now.due_ms) < DRIFT_MS
            });
        if !same {
            self.sends = sends;
        }
        !same
    }

    pub fn sends(&self) -> &[HeldSend] {
        &self.sends
    }

    /// The send the status row shows: the newest, unless the shell is
    /// working on a line, whose thinking or acting shows first.
    pub fn shown_send(&self) -> Option<&HeldSend> {
        match self.state {
            State::Thinking { .. } | State::Acting { .. } => None,
            _ => self.sends.last(),
        }
    }

    /// Send `entry` came to `line` (`failed`: it did not go, or may have).
    /// It leaves the list; and if the line that held it still shows, with
    /// its undo, that line gives way to what came of it. What came of a send
    /// the panel does not know of changes nothing. True when it changed.
    pub fn send_ended(&mut self, entry: u64, line: String, failed: bool) -> bool {
        let before = self.sends.len();
        self.sends.retain(|s| s.entry != entry);
        let mut changed = self.sends.len() != before;
        if self.entry == Some(entry) && matches!(self.state, State::Done { .. }) {
            self.state = match failed {
                true => State::Refused { reason: line },
                false => State::Done { line },
            };
            self.entry = None;
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(entry: u64, sending: bool, left_ms: u64, host: &str) -> WireSend {
        WireSend {
            entry,
            sending,
            left_ms,
            host: String::from(host),
        }
    }

    #[test]
    fn sends_cross_the_wire_and_nothing_else_is_taken_for_them() {
        let sends = alloc::vec![
            wire(12, false, 4_000, "hooks.example"),
            wire(13, true, 0, "10.0.2.2")
        ];
        let text = encode_sends(&sends);
        assert_eq!(
            text,
            "panel sends 12 held 4000 hooks.example;13 sending 0 10.0.2.2"
        );
        assert_eq!(parse_sends(&text), Some(sends));
        assert_eq!(parse_sends(&encode_sends(&[])), Some(Vec::new()));
        for bad in [
            "panel sends ",
            "panel sends 12 held 4000",
            "panel sends 12 gone 4000 a.example",
            "panel sends 12 held 4000 a example",
            "panel sends 12 held 4000 a.example extra",
            "panel sends x held 4000 a.example",
            "panel sends 12 held 4000 a/b",
            "panel state idle",
        ] {
            assert_eq!(parse_sends(bad), None, "{bad}");
        }
        let nine = alloc::vec!["1 held 1 a.example"; MAX_SENDS + 1].join(";");
        assert_eq!(parse_sends(&format!("panel sends {nine}")), None);
    }

    #[test]
    fn what_a_send_came_to_crosses_the_wire() {
        let text = encode_sent(12, false, "Sent to hooks.example.");
        assert_eq!(text, "panel sent 12 ok Sent to hooks.example.");
        assert_eq!(
            parse_sent(&text),
            Some((12, false, String::from("Sent to hooks.example.")))
        );
        let failed = encode_sent(3, true, "Not sent to a.example: 403\nmore");
        assert_eq!(
            parse_sent(&failed),
            Some((3, true, String::from("Not sent to a.example: 403")))
        );
        assert_eq!(parse_sent("panel sent 3 maybe x"), None);
        assert_eq!(parse_sent("panel sent 3 ok "), None);
    }

    #[test]
    fn the_countdown_rounds_up_and_ends_at_zero() {
        assert_eq!(left(10_000, 0), "0:10");
        assert_eq!(left(10_000, 1_000), "0:09");
        assert_eq!(left(10_000, 5_800), "0:05");
        assert_eq!(left(10_000, 10_000), "0:00");
        assert_eq!(left(10_000, 99_000), "0:00");
        assert_eq!(left(75_000, 0), "1:15");
    }

    fn held(entry: u64, due_ms: u64) -> HeldSend {
        wire(entry, false, due_ms, "a.example").held(0)
    }

    /// A line typed during the hold leaves the send shown once it is done.
    #[test]
    fn a_held_send_outlasts_a_new_line() {
        let mut panel = Panel::new();
        assert!(panel.set_sends(alloc::vec![held(5, 5_000)]));
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(5));
        panel.send(String::from("what time is it"), 100);
        assert_eq!(panel.shown_send(), None, "thinking shows first");
        panel.set_done(
            State::Done {
                line: String::from("It is 10:00"),
            },
            None,
        );
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(5));
        assert_eq!(panel.sends().len(), 1);
    }

    #[test]
    fn a_countdown_that_only_drifted_is_no_change() {
        let mut panel = Panel::new();
        assert!(panel.set_sends(alloc::vec![held(5, 5_000)]));
        assert!(!panel.set_sends(alloc::vec![held(5, 5_400)]));
        let mut going = held(5, 5_000);
        going.sending = true;
        assert!(panel.set_sends(alloc::vec![going]));
        assert!(panel.set_sends(Vec::new()));
        assert_eq!(panel.shown_send(), None);
    }

    /// The line that held the send gives way to what came of it; a newer
    /// line's result stands; an outcome for an unknown send changes nothing.
    #[test]
    fn what_a_send_came_to_replaces_only_its_own_line() {
        let mut panel = Panel::new();
        let holding = State::Done {
            line: String::from("Sending to a.example in 5 s. undo 5"),
        };
        panel.set_done(holding, Some(5));
        panel.set_sends(alloc::vec![held(5, 5_000)]);
        assert!(!panel.send_ended(9, String::from("Sent to b.example."), false));
        assert_eq!(panel.sends().len(), 1);
        assert!(panel.send_ended(5, String::from("Sent to a.example."), false));
        assert_eq!(
            panel.state(),
            &State::Done {
                line: String::from("Sent to a.example.")
            }
        );
        assert_eq!(panel.entry(), None, "the sent line offers no undo");
        assert!(panel.sends().is_empty());
        // Held, then a newer line done: what came of the send leaves it.
        panel.set_sends(alloc::vec![held(6, 5_000)]);
        let newer = State::Done {
            line: String::from("Opened Notes"),
        };
        panel.set_done(newer.clone(), Some(7));
        assert!(panel.send_ended(6, String::from("Not sent to a.example: 403"), true));
        assert_eq!(panel.state(), &newer);
        assert_eq!(panel.entry(), Some(7));
        // A failure on its own line shows under the error bar.
        panel.set_done(
            State::Done {
                line: String::from("Sending to a.example in 5 s. undo 8"),
            },
            Some(8),
        );
        panel.send_ended(8, String::from("Not sent to a.example: 403"), true);
        assert!(matches!(panel.state(), State::Refused { .. }));
    }
}
