//! What the screen shows for emoji, which its font has no glyphs for and would
//! otherwise draw as replacement boxes. A smiley becomes its text emoticon and a
//! heart `<3`; other pictographs, and the joiners, selectors and modifiers that
//! build emoji, are left out. Every other character is kept as it is. Only the
//! screen changes: serial keeps the text as printed.

use core::fmt::{self, Write};

/// How the screen shows a character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plain {
    Keep,
    Drop,
    Text(&'static str),
}

/// How the screen shows `ch`.
pub fn plain(ch: char) -> Plain {
    if let Some(text) = emoticon(ch) {
        return Plain::Text(text);
    }
    match ch {
        // Joiners, variation selectors, the keycap mark and tags.
        '\u{200D}' | '\u{FE0E}'..='\u{FE0F}' | '\u{20E3}' | '\u{E0020}'..='\u{E007F}' => {
            Plain::Drop
        }
        // Pictographs (with flags and skin tones), emoticons, transport, the
        // supplements, and the older symbols and dingbats.
        '\u{1F000}'..='\u{1FAFF}' | '\u{2600}'..='\u{27BF}' => Plain::Drop,
        _ => Plain::Keep,
    }
}

/// The text emoticon for a smiley or a heart.
fn emoticon(ch: char) -> Option<&'static str> {
    let text = match ch {
        '\u{263A}'
        | '\u{1F600}'..='\u{1F601}'
        | '\u{1F603}'..='\u{1F604}'
        | '\u{1F607}'
        | '\u{1F60A}'
        | '\u{1F60C}'
        | '\u{1F60E}'
        | '\u{1F642}'
        | '\u{1F917}' => ":)",
        '\u{1F609}' | '\u{1F61C}' => ";)",
        '\u{1F602}' | '\u{1F605}'..='\u{1F606}' | '\u{1F923}' => ":D",
        '\u{1F61B}' | '\u{1F61D}' => ":P",
        '\u{2639}'
        | '\u{1F61E}'..='\u{1F61F}'
        | '\u{1F614}'
        | '\u{1F622}'
        | '\u{1F62D}'
        | '\u{1F641}' => ":(",
        '\u{1F62E}' | '\u{1F632}' => ":O",
        '\u{2665}'
        | '\u{2764}'
        | '\u{1F499}'..='\u{1F49C}'
        | '\u{1F90D}'..='\u{1F90E}'
        | '\u{1F9E1}' => "<3",
        _ => return None,
    };
    Some(text)
}

/// Writes text through to `W` as the screen shows it ([`plain`]).
pub struct Plainly<W>(pub W);

impl<W: Write> Write for Plainly<W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // Kept runs go through whole, so the writer sees few calls.
        let mut kept = 0;
        for (at, ch) in s.char_indices() {
            let shown = plain(ch);
            if shown == Plain::Keep {
                continue;
            }
            self.0.write_str(&s[kept..at])?;
            if let Plain::Text(text) = shown {
                self.0.write_str(text)?;
            }
            kept = at + ch.len_utf8();
        }
        self.0.write_str(&s[kept..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;

    fn shown(text: &str) -> String {
        let mut out = Plainly(String::new());
        out.write_str(text).unwrap();
        out.0
    }

    #[test]
    fn smileys_become_emoticons() {
        assert_eq!(shown("Hello! \u{1F60A}"), "Hello! :)");
        assert_eq!(shown("ok \u{1F609}"), "ok ;)");
        assert_eq!(shown("sorry \u{1F622}"), "sorry :(");
        assert_eq!(shown("ha \u{1F602}"), "ha :D");
    }

    #[test]
    fn a_heart_with_its_selector_is_one_emoticon() {
        assert_eq!(shown("thanks \u{2764}\u{FE0F}"), "thanks <3");
    }

    #[test]
    fn other_pictographs_and_their_parts_are_left_out() {
        assert_eq!(shown("done \u{2705} go \u{1F680}!"), "done  go !");
        // A family: three people joined by zero-width joiners.
        assert_eq!(shown("a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}b"), "ab");
        assert_eq!(shown("\u{1F44D}\u{1F3FD}"), "");
        assert_eq!(shown("\u{1F1FA}\u{1F1F8}"), "");
    }

    #[test]
    fn everything_else_is_kept() {
        let text = "caf\u{E9} 20\u{B0}C 3\u{D7}4 \u{2192} \u{4E2D} \u{FFFD} plain";
        assert_eq!(shown(text), text);
        assert_eq!(shown(""), "");
    }
}
