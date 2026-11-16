//! The sends the shell holds before they go (`shellcore::outbox`, ADR 0002
//! `Hold`), as the Kai pane shows them: "Sending to hooks.example in 0:04"
//! with an `undo` that names the send's log entry, then "Sending to
//! hooks.example…" once the shell says it is going, then code's line for
//! what it came to.
//!
//! They are kept apart from the panel's one state, so a line typed during
//! the hold does not drop them. The status row shows the held send due
//! first while the pane is idle or shows a send's own line; a newer line's
//! result keeps its place, with "· 1 send waiting" after it.
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
    let each: Vec<String> = sends
        .iter()
        .filter(|s| is_host(&s.host))
        .take(MAX_SENDS)
        .map(|s| {
            let stage = if s.sending { "sending" } else { "held" };
            format!("{} {stage} {} {}", s.entry, s.left_ms, s.host)
        })
        .collect();
    match each.is_empty() {
        true => format!("{SENDS}-"),
        false => format!("{SENDS}{}", each.join(";")),
    }
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

/// A host name or address as a send may name one: printable, with no space,
/// `;` or `/`. The same rule as the shell's (`shellcore::outbox::is_host`),
/// which a shellcore test holds equal.
pub fn is_host(host: &str) -> bool {
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

    /// The send the status row shows: the held one due first (else one
    /// going), while the pane is idle or shows a send's own line. A newer
    /// line's result, and thinking or acting, show first.
    pub fn shown_send(&self) -> Option<&HeldSend> {
        let soonest = self
            .sends
            .iter()
            .filter(|s| !s.sending)
            .min_by_key(|s| s.due_ms)
            .or_else(|| self.sends.iter().find(|s| s.sending))?;
        let own_line = self.send_state
            || self
                .entry
                .is_some_and(|entry| self.sends.iter().any(|s| s.entry == entry));
        match self.state {
            State::Idle => Some(soonest),
            State::Done { .. } | State::Refused { .. } if own_line => Some(soonest),
            _ => None,
        }
    }

    /// "1 send waiting", after a newer line's result that shows in place of
    /// the held sends; `None` when they show, or none waits.
    pub fn waiting_note(&self) -> Option<String> {
        let busy = matches!(self.state, State::Thinking { .. } | State::Acting { .. });
        if self.sends.is_empty() || busy || self.shown_send().is_some() {
            return None;
        }
        Some(match self.sends.len() {
            1 => String::from("1 send waiting"),
            n => format!("{n} sends waiting"),
        })
    }

    /// Send `entry` came to `line` (`failed`: it did not go, or may have).
    /// It leaves the list; and if the line that held it still shows, with
    /// its undo, that line gives way to what came of it. What came of a send
    /// the panel does not know of changes nothing. True when it changed.
    pub fn send_ended(&mut self, entry: u64, line: String, failed: bool) -> bool {
        let before = self.sends.len();
        self.sends.retain(|s| s.entry != entry);
        let mut changed = self.sends.len() != before;
        let own = self.entry == Some(entry) || self.send_state;
        if own && matches!(self.state, State::Done { .. } | State::Refused { .. }) {
            self.state = match failed {
                true => State::Refused { reason: line },
                false => State::Done { line },
            };
            self.entry = None;
            self.send_state = true;
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

    fn held_send(entry: u64, due_ms: u64) -> HeldSend {
        held(entry, due_ms)
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
        assert_eq!(panel.shown_send(), None, "the new line's result shows");
        assert_eq!(panel.waiting_note().as_deref(), Some("1 send waiting"));
        assert_eq!(panel.sends().len(), 1);
        panel.set_state(State::Idle);
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(5));
        assert_eq!(panel.waiting_note(), None);
    }

    /// The row shows the held send due first, not the newest (#959 review).
    #[test]
    fn the_send_due_first_is_the_one_shown() {
        let mut panel = Panel::new();
        panel.set_sends(alloc::vec![held(4, 5_000), held(6, 7_000)]);
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(4));
        panel.set_sends(alloc::vec![held(6, 7_000), held(4, 5_000)]);
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(4));
        let mut going = held(4, 0);
        going.sending = true;
        panel.set_sends(alloc::vec![going, held(6, 7_000)]);
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(6));
        let mut alone = held(4, 0);
        alone.sending = true;
        panel.set_sends(alloc::vec![alone]);
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(4));
    }

    /// Once one send ends on its own line, the next held send shows there.
    #[test]
    fn after_one_send_ends_the_next_shows() {
        let mut panel = Panel::new();
        panel.set_done(
            State::Done {
                line: String::from("Sending to a.example in 5 s. undo 4"),
            },
            Some(4),
        );
        panel.set_sends(alloc::vec![held(4, 5_000), held(6, 7_000)]);
        assert!(panel.send_ended(4, String::from("Sent to a.example."), false));
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(6));
        assert!(panel.send_ended(6, String::from("Sent to b.example."), false));
        assert_eq!(
            panel.state(),
            &State::Done {
                line: String::from("Sent to b.example.")
            }
        );
    }

    /// A send allowed on the trusted card comes back as the card's result
    /// over the real wire, with its entry: the pane shows its countdown and
    /// undo as that line's own, and what it came to replaces the line
    /// (#959 re-review).
    #[test]
    fn a_send_allowed_on_the_card_shows_as_its_own_line() {
        use crate::panel_wire::{encode_card_held, parse_card_held};
        let mut panel = Panel::new();
        panel.send(String::from("send hello to a.example"), 0);
        let line = State::Done {
            line: String::from("Sending to a.example in 5 s."),
        };
        let request = encode_card_held(&line, Some(4)).unwrap();
        assert_eq!(request, "panel card done Sending to a.example in 5 s.\t4");
        let (shown, held) = parse_card_held(&request).unwrap();
        assert_eq!(held, Some(4));
        panel.set_done(shown, held);
        panel.set_sends(alloc::vec![held_send(4, 5_000)]);
        assert_eq!(panel.shown_send().map(|s| s.entry), Some(4));
        assert_eq!(panel.waiting_note(), None);
        assert!(panel.send_ended(4, String::from("Sent to a.example."), false));
        assert_eq!(
            panel.state(),
            &State::Done {
                line: String::from("Sent to a.example.")
            }
        );
        // A card that held nothing reads back as before.
        let plain = encode_card_held(&line, None).unwrap();
        assert_eq!(parse_card_held(&plain), Some((line.clone(), None)));
        assert_eq!(crate::panel_wire::parse_card(&request), Some(line));
    }

    #[test]
    fn a_bad_host_never_reaches_the_wire() {
        let sends = alloc::vec![wire(1, false, 1, "a;b c"), wire(2, false, 1, "b.example")];
        assert_eq!(encode_sends(&sends), "panel sends 2 held 1 b.example");
        assert_eq!(encode_sends(&sends[..1]), "panel sends -");
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
