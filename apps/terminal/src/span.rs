//! The scrollback's rows. The kernel console colours a row by what its text
//! looks like (`termcore::style::Kind::of`), so a program that prints `= ok`
//! looks like the shell. Here every row is typed by where it came from: what the
//! user or the AI typed, or which kind of reply the shell sent. Nothing in a
//! reply's text can change its kind (ADR 0005 section 4).

use alloc::string::String;
use alloc::vec::Vec;
use minicbor::{Decode, Encode};
use termcore::plain::{Plain, plain};

/// Rows kept. With [`MAX_LINE_BYTES`] this keeps the saved state under the 64 KiB
/// checkpoint, whatever the characters are (a test holds it to that).
pub const MAX_SPANS: usize = 200;
/// Longest row, in bytes; a longer line is cut at a character and ends in `…`.
pub const MAX_LINE_BYTES: usize = 240;
/// Most lines of one reply that are kept.
pub const MAX_REPLY_LINES: usize = 64;
/// Most bytes of one reply that are read, as long as one message (`kabi::MAX_MESSAGE`).
pub const MAX_REPLY_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
#[cbor(index_only)]
pub enum Kind {
    /// A line typed at a prompt, by the user or by the AI.
    #[n(0)]
    Input,
    /// `ok`: an answer from the shell.
    #[n(1)]
    Reply,
    /// `err`: an error from the shell.
    #[n(2)]
    Error,
    /// `confirm` and `menu`: a question the trusted path answers.
    #[n(3)]
    Question,
    /// The terminal's own words.
    #[n(4)]
    Notice,
}

impl Kind {
    /// The word `terminal_window_read` and the agent's view use.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Reply => "reply",
            Self::Error => "error",
            Self::Question => "question",
            Self::Notice => "notice",
        }
    }

    /// The mark before the row's text, as on the console: `= `, `! ` and `? `.
    /// It is drawn, not part of the text, so a reply cannot fake one.
    pub const fn lead(self) -> &'static str {
        match self {
            Self::Reply => "= ",
            Self::Error => "! ",
            Self::Question => "? ",
            Self::Input | Self::Notice => "",
        }
    }
}

/// One row of the scrollback.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Span {
    /// Counts up from 1 for the life of the terminal, so a reader can ask for
    /// the rows after the last one it saw.
    #[n(0)]
    pub seq: u32,
    #[n(1)]
    pub kind: Kind,
    /// The prompt an input was typed at; empty for every other kind.
    #[n(2)]
    pub prompt: String,
    #[n(3)]
    pub text: String,
    /// Whether the AI typed the line (an input), through `terminal_window_run`.
    #[n(4)]
    pub by_ai: bool,
}

/// Text from the shell made safe to show: tabs become spaces, other control
/// characters go, and emoji the font has no glyphs for become their emoticon
/// or go (`termcore::plain`). Split into lines, each cut to [`MAX_LINE_BYTES`],
/// with at most [`MAX_REPLY_LINES`] of them.
pub fn clean(text: &str) -> Cleaned {
    let mut shown = String::new();
    let mut bytes = 0;
    let mut cut = false;
    for ch in text.chars() {
        bytes += ch.len_utf8();
        if bytes > MAX_REPLY_BYTES {
            cut = true;
            break;
        }
        match ch {
            '\n' => shown.push('\n'),
            '\t' => shown.push(' '),
            c if c.is_control() || hidden(c) => {}
            c => match plain(c) {
                Plain::Keep => shown.push(c),
                Plain::Text(t) => shown.push_str(t),
                Plain::Drop => {}
            },
        }
    }
    let mut lines: Vec<String> = shown.split('\n').map(fit).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.len() > MAX_REPLY_LINES {
        lines.truncate(MAX_REPLY_LINES);
        cut = true;
    }
    Cleaned { lines, cut }
}

/// What [`clean`] kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cleaned {
    pub lines: Vec<String>,
    /// Whether lines or bytes past the limits were left out.
    pub cut: bool,
}

/// Characters that draw nothing or reorder what is drawn around them: the
/// zero-width and bidirectional controls, the line and paragraph separators and
/// the byte-order mark. A row never shows them.
fn hidden(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
    )
}

/// `text` without control characters or [`hidden`] ones, blanks kept.
pub fn visible(text: &str) -> String {
    text.chars()
        .filter(|&c| !c.is_control() && !hidden(c))
        .collect()
}

/// `line` without trailing blanks, cut to [`MAX_LINE_BYTES`] with a `…`.
pub fn fit(line: &str) -> String {
    let line = line.trim_end();
    if line.len() <= MAX_LINE_BYTES {
        return String::from(line);
    }
    let room = MAX_LINE_BYTES - '…'.len_utf8();
    let mut out = String::new();
    for ch in line.chars() {
        if out.len() + ch.len_utf8() > room {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// `line` in rows of at most `width` characters, broken at the last space that
/// fits so words stay whole. A word longer than `width` is cut.
pub fn wrap(line: &str, width: usize) -> Vec<&str> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut rest = line;
    loop {
        let Some((cut, next)) = rest.char_indices().nth(width) else {
            rows.push(rest);
            return rows;
        };
        // A space just past the row ends a word too.
        let space = rest[..cut + next.len_utf8()]
            .rfind(' ')
            .filter(|&at| at > 0);
        let (row, after) = match space {
            Some(at) => (&rest[..at], &rest[at + 1..]),
            None => (&rest[..cut], &rest[cut..]),
        };
        rows.push(row);
        rest = after;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_drops_controls_and_maps_emoji() {
        let c = clean("a\tb\u{1b}[31mred\r\n\u{1F600} done\u{1F680}");
        assert_eq!(c.lines, ["a b[31mred", ":) done"]);
        assert!(!c.cut);
    }

    #[test]
    fn clean_drops_characters_that_hide_or_reorder_text() {
        let c = clean("a\u{200B}b\u{202E}c\u{2066}d\u{FEFF}e\u{2028}f");
        assert_eq!(c.lines, ["abcdef"]);
    }

    #[test]
    fn clean_cuts_long_lines_and_many_lines() {
        let long = "x".repeat(MAX_LINE_BYTES + 10);
        let c = clean(&long);
        assert_eq!(c.lines[0].len(), MAX_LINE_BYTES);
        assert!(c.lines[0].ends_with('…'));
        // Wide characters are cut whole, never inside.
        let wide = "\u{20ac}".repeat(MAX_LINE_BYTES);
        let c = clean(&wide);
        assert!(c.lines[0].len() <= MAX_LINE_BYTES && c.lines[0].ends_with('…'));
        assert!(
            c.lines[0]
                .trim_end_matches('…')
                .chars()
                .all(|ch| ch == '\u{20ac}')
        );
        let many = "l\n".repeat(MAX_REPLY_LINES + 5);
        let c = clean(&many);
        assert_eq!(c.lines.len(), MAX_REPLY_LINES);
        assert!(c.cut);
    }

    #[test]
    fn clean_reads_no_more_than_one_message() {
        let c = clean(&"y".repeat(MAX_REPLY_BYTES * 2));
        assert!(c.cut);
        assert!(c.lines.iter().map(|l| l.len()).sum::<usize>() <= MAX_REPLY_BYTES);
    }

    #[test]
    fn trailing_blank_lines_go_but_inner_ones_stay() {
        assert_eq!(clean("a\n\nb\n\n  \n").lines, ["a", "", "b"]);
        assert!(clean("").lines.is_empty());
    }

    #[test]
    fn wrap_breaks_at_spaces_and_cuts_long_words() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("", 5), [""]);
        assert_eq!(wrap("ab cd", 5), ["ab cd"]);
    }
}
