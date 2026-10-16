//! Making server-supplied text safe to show on the console and the serial line.

use alloc::string::String;

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
}
