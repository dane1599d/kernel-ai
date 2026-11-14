//! HID keyboard usages (page 0x07, USB HID Usage Tables 1.5 §10) as the text
//! they type, US layout: `desktop` gives every key its text, so the panel and
//! native apps need no keymap of their own (ADR 0005 §3).

use termcore::keys::Key;

const LETTERS: u16 = 0x04;
const DIGITS: u16 = 0x1E;
const SHIFTED_DIGITS: &[u8; 10] = b"!@#$%^&*()";

/// Caps Lock, as a key.
pub const CAPS_LOCK: u16 = 0x39;
/// Num Lock, as a key.
pub const NUM_LOCK: u16 = 0x53;

/// A keypad key as the key it is with Num Lock off: the arrows and
/// navigation keys. `None` for one that is the same either way, or none
/// with Num Lock off (keypad 5).
pub fn keypad_off(usage: u16) -> Option<u16> {
    Some(match usage {
        0x59 => 0x4D, // 1: End
        0x5A => 0x51, // 2: Down
        0x5B => 0x4E, // 3: Page Down
        0x5C => 0x50, // 4: Left
        0x5E => 0x4F, // 6: Right
        0x5F => 0x4A, // 7: Home
        0x60 => 0x52, // 8: Up
        0x61 => 0x4B, // 9: Page Up
        0x62 => 0x49, // 0: Insert
        0x63 => 0x4C, // .: Delete
        _ => return None,
    })
}

/// The character `usage` types with Shift `shift` and Caps Lock `caps`, if
/// it types one. Caps Lock turns letters only, the other way from Shift.
pub fn char_of(usage: u16, shift: bool, caps: bool) -> Option<char> {
    let ch = match usage {
        LETTERS..=0x1D => {
            let c = b'a' + (usage - LETTERS) as u8;
            if shift != caps {
                c.to_ascii_uppercase()
            } else {
                c
            }
        }
        // The keypad, as Num Lock on sends it: digits and operators.
        0x54 => b'/',
        0x55 => b'*',
        0x56 => b'-',
        0x57 => b'+',
        0x59..=0x61 => b'1' + (usage - 0x59) as u8,
        0x62 => b'0',
        0x63 => b'.',
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
                // ISO keyboards' two extra keys, in the US layout.
                0x32 => (b'#', b'~'),
                0x64 => (b'\\', b'|'),
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
pub fn key_of(usage: u16, shift: bool, ctrl: bool, caps: bool) -> Option<Key> {
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
            let ch = char_of(usage, shift, caps)?;
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
        assert_eq!(char_of(0x04, false, false), Some('a'));
        assert_eq!(char_of(0x1D, true, false), Some('Z'));
        assert_eq!(char_of(0x1E, false, false), Some('1'));
        assert_eq!(char_of(0x27, false, false), Some('0'));
        assert_eq!(char_of(0x27, true, false), Some(')'));
        assert_eq!(char_of(0x2C, false, false), Some(' '));
        assert_eq!(char_of(0x38, true, false), Some('?'));
        assert_eq!(
            char_of(0xE1, false, false),
            None,
            "Shift itself types nothing"
        );
    }

    #[test]
    fn editing_keys_and_ctrl_letters_reach_the_line_editor() {
        assert_eq!(key_of(0x28, false, false, false), Some(Key::Enter));
        assert_eq!(key_of(0x50, false, false, false), Some(Key::Left));
        assert_eq!(key_of(0x06, false, true, false), Some(Key::Ctrl('c')));
        assert_eq!(key_of(0x06, true, false, false), Some(Key::Char('C')));
        assert_eq!(key_of(0x1E, false, true, false), None);
        assert_eq!(key_of(0xE0, false, false, false), None);
    }

    #[test]
    fn caps_lock_turns_letters_only_and_the_keypad_and_iso_keys_type() {
        assert_eq!(char_of(0x04, false, true), Some('A'));
        assert_eq!(
            char_of(0x04, true, true),
            Some('a'),
            "Shift undoes Caps Lock"
        );
        assert_eq!(char_of(0x1E, false, true), Some('1'), "not digits");
        assert_eq!(char_of(0x59, false, false), Some('1'));
        assert_eq!(char_of(0x62, false, false), Some('0'));
        assert_eq!(char_of(0x57, false, false), Some('+'));
        assert_eq!(char_of(0x63, false, false), Some('.'));
        assert_eq!(char_of(0x64, false, false), Some('\\'));
        assert_eq!(char_of(0x32, true, false), Some('~'));
        assert_eq!(keypad_off(0x5C), Some(0x50), "keypad 4 is Left");
        assert_eq!(keypad_off(0x5D), None);
        assert_eq!(key_of(0x58, false, false, false), Some(Key::Enter));
    }
}
