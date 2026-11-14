//! The panel's history on the desk (the founder, 2026-11-14): each panel line
//! and what came of it, above the bar, newest at the bottom, as a chat shows
//! them. It is drawn on the desk's background, under every window, and never
//! inside the bar, whose state line stays code's alone (ADR 0005 §8).
//!
//! Three kinds of entry, each drawn in its own look ([`Who`]):
//! - the person's line, as `desktop` took it for the shell;
//! - what the AI said (`panel said`, `panel_wire`): the model's words or its
//!   script's prints. Not the system's: drawn in the AI's look, apart from
//!   the system's mono, with the `Kai` label, so the model's text can never
//!   pass for the system's own;
//! - the bar's result line or refusal, code's, as a note.
//!
//! Bounded: at most [`MAX_ENTRIES`], each at most [`MAX_TEXT`] bytes, with
//! control characters dropped.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// Entries kept; the oldest goes first.
pub const MAX_ENTRIES: usize = 40;
/// Bytes kept of one entry's text.
pub const MAX_TEXT: usize = 4000;

/// Who an entry is from, which decides its look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// The person's panel line.
    You,
    /// The AI's words or its script's prints: never the system's look.
    Kai,
    /// Code's result line or refusal, as the bar showed it.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its number, from 0, never reused: what a painter's laid-out lines are
    /// kept by.
    pub seq: u64,
    pub who: Who,
    pub text: String,
}

/// The history.
#[derive(Default)]
pub struct Chat {
    entries: VecDeque<Entry>,
    next: u64,
}

impl Chat {
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds `text` from `who`, made safe to draw; false when nothing is left
    /// of it.
    pub fn push(&mut self, who: Who, text: &str) -> bool {
        let text = clean(text);
        if text.is_empty() {
            return false;
        }
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            seq: self.next,
            who,
            text,
        });
        self.next += 1;
        true
    }
}

/// `text` with control characters other than newlines dropped, tabs as a
/// space, blank lines at either end trimmed, cut to [`MAX_TEXT`] bytes at a
/// character boundary.
fn clean(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            '\n' => Some('\n'),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    let kept = kept.trim_matches(|c: char| c == '\n' || c == ' ');
    let mut end = kept.len().min(MAX_TEXT);
    while !kept.is_char_boundary(end) {
        end -= 1;
    }
    String::from(&kept[..end])
}

/// `text` broken into lines no wider than `width` by `measure`: at its own
/// newlines, then between words, and inside a word too wide for a line.
pub fn wrap(text: &str, width: f32, measure: &mut dyn FnMut(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let start = lines.len();
        let mut line = String::new();
        for word in para.split(' ') {
            let joined = match line.is_empty() {
                true => String::from(word),
                false => alloc::format!("{line} {word}"),
            };
            if measure(&joined) <= width {
                line = joined;
                continue;
            }
            if !line.is_empty() {
                lines.push(core::mem::take(&mut line));
            }
            // A word too wide for a line alone is broken where it must be.
            let mut rest = word;
            loop {
                let cut = fit(rest, width, measure);
                if cut == rest.len() {
                    break;
                }
                lines.push(String::from(&rest[..cut]));
                rest = &rest[cut..];
            }
            line = String::from(rest);
        }
        // A blank paragraph is a blank line; a broken word's end leaves none.
        if !line.is_empty() || lines.len() == start {
            lines.push(line);
        }
    }
    lines
}

/// How many bytes of `text` fit in `width`, at a character: all of it when
/// it fits, else at least one character. It measures prefixes of growing
/// size, doubling, then halves the step between the last that fitted and the
/// first that did not, so no prefix measured is much longer than a line:
/// an unbroken string costs a few measures a line, not one a character.
fn fit(text: &str, width: f32, measure: &mut dyn FnMut(&str) -> f32) -> usize {
    let bounds: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .skip(1)
        .chain(core::iter::once(text.len()))
        .collect();
    // `bounds[i]` is the end of the first `i + 1` characters.
    let fits =
        |i: usize, measure: &mut dyn FnMut(&str) -> f32| measure(&text[..bounds[i]]) <= width;
    let last = bounds.len() - 1;
    let (mut good, mut probe) = (None, 0);
    let bad = loop {
        if !fits(probe, measure) {
            break probe;
        }
        if probe == last {
            return text.len();
        }
        good = Some(probe);
        probe = (probe * 2 + 1).min(last);
    };
    // Between the last that fitted and the first that did not.
    let mut lo = good.map_or(0, |g| g + 1);
    let mut hi = bad;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if fits(mid, measure) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    // `lo` characters fit (none fitted: one anyway).
    match lo {
        0 => bounds[0],
        n => bounds[n - 1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One unit a character, as a mono face measures.
    fn cols(text: &str) -> f32 {
        text.chars().count() as f32
    }

    #[test]
    fn the_history_keeps_the_newest_and_drops_control_characters() {
        let mut chat = Chat::default();
        assert!(!chat.push(Who::Kai, "\n \u{1b}\n"));
        assert!(chat.is_empty());
        for n in 0..MAX_ENTRIES + 3 {
            chat.push(Who::You, &alloc::format!("line {n}"));
        }
        let first = chat.entries().next().unwrap();
        assert_eq!(first.text, "line 3");
        assert_eq!(chat.entries().count(), MAX_ENTRIES);
        chat.push(Who::Kai, "a\u{1b}[31mred\tb\n\nc\n");
        let last = chat.entries().next_back().unwrap();
        assert_eq!(
            (last.who, last.text.as_str()),
            (Who::Kai, "a[31mred b\n\nc")
        );
    }

    #[test]
    fn a_long_entry_is_cut_at_a_character() {
        let mut chat = Chat::default();
        chat.push(Who::Kai, &"é".repeat(MAX_TEXT));
        let text = &chat.entries().next().unwrap().text;
        assert!(text.len() <= MAX_TEXT && text.chars().all(|c| c == 'é'));
    }

    /// #859's review: an unbroken string (a hash, base64) wraps in a few
    /// measures a line, each of about a line's length.
    #[test]
    fn a_long_unbroken_string_wraps_with_few_short_measures() {
        let text = "x".repeat(MAX_TEXT);
        let mut calls = 0usize;
        let mut measure = |t: &str| {
            calls += 1;
            t.len() as f32
        };
        let lines = wrap(&text, 80.0, &mut measure);
        assert_eq!(lines.len(), MAX_TEXT / 80);
        assert!(lines.iter().all(|l| l.len() == 80));
        // A character at a time was 4,000 measures a line here.
        assert!(calls < 20 * lines.len(), "{calls} measures");
    }

    #[test]
    fn wrapping_breaks_between_words_and_inside_a_long_one() {
        let lines = wrap("the quick brown fox", 9.0, &mut cols);
        assert_eq!(lines, ["the quick", "brown fox"]);
        let lines = wrap("a\nbb cc", 5.0, &mut cols);
        assert_eq!(lines, ["a", "bb cc"]);
        let lines = wrap("abcdefghij k", 4.0, &mut cols);
        assert_eq!(lines, ["abcd", "efgh", "ij k"]);
        assert!(wrap("x y z", 0.5, &mut cols).iter().all(|l| !l.is_empty()));
    }
}
