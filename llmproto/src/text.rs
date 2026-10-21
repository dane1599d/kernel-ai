//! Making server-supplied text safe to show on the console and the serial line.

use alloc::string::String;
use alloc::vec::Vec;

/// Server text made safe to print. Tabs become spaces; every other control
/// character except newline is dropped, since carriage returns, backspaces and
/// escape sequences could redraw the screen or the host terminal. The result has
/// at most `max_chars` characters and `max_lines` lines.
pub fn printable(text: &str, max_chars: usize, max_lines: usize) -> String {
    let mut lines = 1;
    text.chars()
        .map(|ch| if ch == '\t' { ' ' } else { ch })
        .filter(|&ch| ch == '\n' || !ch.is_control())
        .take_while(|&ch| {
            if ch == '\n' {
                lines += 1;
            }
            lines <= max_lines
        })
        .take(max_chars)
        .collect()
}

/// `text` trimmed and without the `<think>...</think>` block a model that thinks
/// anyway puts first; empty when the block never ends.
pub fn without_thinking(text: &str) -> &str {
    let text = text.trim();
    match text.strip_prefix("<think>") {
        Some(rest) => rest
            .split_once("</think>")
            .map_or("", |(_, after)| after)
            .trim(),
        None => text,
    }
}

/// `line` in rows of at most `width` characters, broken at the last space that
/// fits so words stay whole; the space a row breaks at is dropped. A word longer
/// than `width` is cut. An empty line is one empty row.
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
        let (row, next) = match space {
            Some(at) => (&rest[..at], &rest[at + 1..]),
            None => (&rest[..cut], &rest[cut..]),
        };
        rows.push(row);
        rest = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn drops_control_characters_but_keeps_newlines() {
        let escape = char::from(27u8);
        let backspace = char::from(8u8);
        let csi = char::from_u32(0x9B).unwrap();
        let hostile = [
            "a",
            "\r",
            &escape.to_string(),
            "[2Jb",
            &backspace.to_string(),
            "\tc\nd",
            &csi.to_string(),
        ]
        .concat();
        assert_eq!(printable(&hostile, 100, 10), "a[2Jb c\nd");
    }

    #[test]
    fn caps_characters_and_lines() {
        assert_eq!(printable("abcdef", 3, 10), "abc");
        assert_eq!(printable("1\n2\n3\n4", 100, 2), "1\n2");
        assert_eq!(printable("\n\n\n\n", 100, 3), "\n\n");
        assert_eq!(printable("héllo", 2, 1), "hé");
    }

    #[test]
    fn wraps_at_spaces_and_cuts_only_long_words() {
        assert_eq!(wrap("", 5), [""]);
        assert_eq!(wrap("short", 5), ["short"]);
        assert_eq!(
            wrap("It controls the CPUs", 9),
            ["It", "controls", "the CPUs"]
        );
        assert_eq!(wrap("aaaa bbbb", 4), ["aaaa", "bbbb"]);
        assert_eq!(wrap("abcdefgh ij", 3), ["abc", "def", "gh", "ij"]);
        assert_eq!(wrap(" 1 | x", 10), [" 1 | x"]);
        assert_eq!(wrap("héllo wörld", 6), ["héllo", "wörld"]);
        assert_eq!(wrap("ab éé", 3), ["ab", "éé"]);
        assert_eq!(wrap("abéé", 2), ["ab", "éé"]);
        for rows in [
            wrap("one two three four five six", 7),
            wrap("x".repeat(20).as_str(), 7),
        ] {
            assert!(rows.iter().all(|row| row.chars().count() <= 7), "{rows:?}");
        }
    }
}
