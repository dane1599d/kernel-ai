//! An ask card's text (ADR 0005 §7, §8): the call exactly as the kernel hashes
//! it into the `ONCE` digest, the reason, and the time left. `desktop` draws
//! only what the kernel handed it. The call's canonical text is shown whole,
//! wrapped at the card's width without dropping or changing a character, except
//! that a character outside ASCII shows as a `\u{...}` escape, so a look-alike
//! letter cannot pass for another.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// How long an ask waits for its answer (`shellcore/src/actions.rs`'s 2:00).
pub const WAIT_MS: u64 = 120_000;

/// One ask, as the kernel posted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// The log entry it asks for (`actlog 1`): what its answer names.
    pub id: u64,
    /// The call in canonical text (`termcore::once::call_text`).
    pub call: String,
    /// Why the agent wants it, in the agent's words.
    pub reason: String,
    /// When it was shown, for the countdown.
    pub shown_ms: u64,
}

/// `text` as the card shows it: ASCII as it is, anything else as `\u{hex}`.
pub fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() && !ch.is_ascii_control() {
            out.push(ch);
        } else {
            out.push_str(&format!("\\u{{{:x}}}", ch as u32));
        }
    }
    out
}

/// `text` cut into lines of at most `cols` characters, every character kept.
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return alloc::vec![String::new()];
    }
    chars.chunks(cols).map(|c| c.iter().collect()).collect()
}

impl Card {
    /// The call's lines and the reason's, at `cols` characters wide.
    pub fn lines(&self, cols: usize) -> (Vec<String>, Vec<String>) {
        let reason = match self.reason.is_empty() {
            true => Vec::new(),
            false => wrap(&shown(&self.reason), cols),
        };
        (wrap(&shown(&self.call), cols), reason)
    }

    /// The countdown at `now_ms`, as `m:ss`.
    pub fn left(&self, now_ms: u64) -> String {
        let left = WAIT_MS.saturating_sub(now_ms.saturating_sub(self.shown_ms)) / 1000;
        format!("{}:{:02}", left / 60, left % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_shows_escaped_and_ascii_as_is() {
        assert_eq!(
            shown("process_stop(\"build-3\")"),
            "process_stop(\"build-3\")"
        );
        // A Cyrillic "а" looks like a Latin "a".
        assert_eq!(shown("n\u{430}tes"), "n\\u{430}tes");
        assert_eq!(shown("tab\there"), "tab\\u{9}here");
    }

    #[test]
    fn wrapping_keeps_every_character() {
        let text = "editor_open(\"notes/src/lib.rs\", 40)";
        let lines = wrap(text, 10);
        assert!(lines.iter().all(|l| l.chars().count() <= 10));
        assert_eq!(lines.concat(), text);
        assert_eq!(wrap("", 10), [String::new()]);
    }

    #[test]
    fn the_countdown_runs_from_two_minutes_to_zero() {
        let card = Card {
            id: 7,
            call: "x()".into(),
            reason: String::new(),
            shown_ms: 1_000,
        };
        assert_eq!(card.left(1_000), "2:00");
        assert_eq!(card.left(62_500), "0:58");
        assert_eq!(card.left(500_000), "0:00");
        assert_eq!(card.lines(20).1.len(), 0);
    }
}
