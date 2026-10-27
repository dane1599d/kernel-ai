//! HID keyboard usages (page 0x07, USB HID Usage Tables 1.5 §10) as the text
//! they type, US layout: `desktop` gives every key its text, so the panel and
//! native apps need no keymap of their own (ADR 0005 §3).

use termcore::keys::Key;

const LETTERS: u16 = 0x04;
const DIGITS: u16 = 0x1E;
const SHIFTED_DIGITS: &[u8; 10] = b"!@#$%^&*()";

/// The character `usage` types with Shift `shift`, if it types one.
pub fn char_of(usage: u16, shift: bool) -> Option<char> {
    let ch = match usage {
        LETTERS..=0x1D => {
            let c = b'a' + (usage - LETTERS) as u8;
            if shift { c.to_ascii_uppercase() } else { c }
        }
        DIGITS..=0x27 => {
            let n = (usage - DIGITS) as usize;
            match (shift, n) {
                (true, n) => SHIFTED_DIGITS[n],
                (false, 9) => b'0',
                (false, n) => b'1' + n as u8,
            }
        }
        0x2C => b' ',
        _ => {
            let (plain, shifted) = match usage {
                0x2D => (b'-', b'_'),
                0x2E => (b'=', b'+'),
                0x2F => (b'[', b'{'),
                0x30 => (b']', b'}'),
                0x31 => (b'\\', b'|'),
                0x33 => (b';', b':'),
                0x34 => (b'\'', b'"'),
                0x35 => (b'`', b'~'),
                0x36 => (b',', b'<'),
                0x37 => (b'.', b'>'),
                0x38 => (b'/', b'?'),
                _ => return None,
            };
            if shift { shifted } else { plain }
        }
    };
    Some(char::from(ch))
}

/// `usage` as the line editor's key, with Shift and Ctrl as held: a
/// character, an editing key, or `Ctrl` with a letter. None for keys the
/// editor ignores (modifiers, function keys).
pub fn key_of(usage: u16, shift: bool, ctrl: bool) -> Option<Key> {
    let named = match usage {
        0x28 | 0x58 => Key::Enter,
        0x29 => Key::Esc,
        0x2A => Key::Backspace,
        0x2B => Key::Tab,
        0x4A => Key::Home,
        0x4B => Key::PageUp,
        0x4C => Key::Delete,
        0x4D => Key::End,
        0x4E => Key::PageDown,
        0x4F => Key::Right,
        0x50 => Key::Left,
        0x51 => Key::Down,
        0x52 => Key::Up,
        _ => {
            let ch = char_of(usage, shift)?;
            return Some(match ctrl {
                true if ch.is_ascii_alphabetic() => Key::Ctrl(ch.to_ascii_lowercase()),
                true => return None,
                false => Key::Char(ch),
            });
        }
    };
    Some(named)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_symbols_type_their_us_characters() {
        assert_eq!(char_of(0x04, false), Some('a'));
        assert_eq!(char_of(0x1D, true), Some('Z'));
        assert_eq!(char_of(0x1E, false), Some('1'));
        assert_eq!(char_of(0x27, false), Some('0'));
        assert_eq!(char_of(0x27, true), Some(')'));
        assert_eq!(char_of(0x2C, false), Some(' '));
        assert_eq!(char_of(0x38, true), Some('?'));
        assert_eq!(char_of(0xE1, false), None, "Shift itself types nothing");
    }

    #[test]
    fn editing_keys_and_ctrl_letters_reach_the_line_editor() {
        assert_eq!(key_of(0x28, false, false), Some(Key::Enter));
        assert_eq!(key_of(0x50, false, false), Some(Key::Left));
        assert_eq!(key_of(0x06, false, true), Some(Key::Ctrl('c')));
        assert_eq!(key_of(0x06, true, false), Some(Key::Char('C')));
        assert_eq!(key_of(0x1E, false, true), None);
        assert_eq!(key_of(0xE0, false, false), None);
    }
}
