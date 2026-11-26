//! HID keyboard usages (page 0x07, USB HID Usage Tables 1.5 §10) as the text
//! they type, US layout: `desktop` gives every key its text, so the panel and
//! native apps need no keymap of their own (ADR 0005 §3).
//!
//! [`Layout`] holds the other layouts as data (plan L-2, the host half): one
//! row per key of the main block, in [`crate::keymaps`]. Choosing one in
//! Settings, and AltGr and dead keys in `desktop`, come later; until then
//! every key types its US text through [`char_of`].

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

/// What one level of a key types: a character, an accent that waits for the
/// next key (a dead key), or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sym {
    None,
    Char(char),
    Dead(Accent),
}

/// A dead key's accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    Grave,
    Acute,
    Circumflex,
    Tilde,
    Diaeresis,
}

/// Each accent: its spacing character, then the letters it goes on and what
/// they become, in step.
const ACCENTS: [(Accent, char, &str, &str); 5] = [
    (Accent::Grave, '`', "aeiouAEIOU", "àèìòùÀÈÌÒÙ"),
    (Accent::Acute, '´', "aeiouyAEIOUY", "áéíóúýÁÉÍÓÚÝ"),
    (Accent::Circumflex, '^', "aeiouAEIOU", "âêîôûÂÊÎÔÛ"),
    (Accent::Tilde, '~', "anoANO", "ãñõÃÑÕ"),
    (Accent::Diaeresis, '¨', "aeiouyAEIOUY", "äëïöüÿÄËÏÖÜŸ"),
];

impl Accent {
    /// The accent typed on its own: the dead key then Space, or twice.
    pub fn spacing(self) -> char {
        ACCENTS.iter().find(|a| a.0 == self).map_or(' ', |a| a.1)
    }
}

/// What a dead key's `accent` and then `next` type as one character: the
/// accented letter, or the accent itself after Space or the same dead key.
/// None when the two do not combine; the accent and then `next` are typed.
pub fn compose(accent: Accent, next: Sym) -> Option<char> {
    let (_, spacing, from, to) = ACCENTS.iter().find(|a| a.0 == accent)?;
    match next {
        Sym::Char(' ') => Some(*spacing),
        Sym::Dead(again) if again == accent => Some(*spacing),
        Sym::Char(base) => from
            .chars()
            .zip(to.chars())
            .find(|p| p.0 == base)
            .map(|p| p.1),
        _ => None,
    }
}

/// One key of a layout: its usage, then what it types plain, with Shift and
/// with AltGr (the right Alt key).
pub type Row = (u16, Sym, Sym, Sym);

/// A keyboard layout as data: what each key of the main block types. Space
/// and the keypad are the same in every layout but for the keypad's decimal
/// key; the keys this does not name type nothing.
#[derive(Debug)]
pub struct Layout {
    /// The short name a setting would hold: `us`, `gb`, `de`, ...
    pub id: &'static str,
    /// The name Settings would show.
    pub name: &'static str,
    pub rows: &'static [Row],
    /// What the keypad's decimal key types with Num Lock on.
    pub keypad_decimal: char,
}

/// Every layout, US first.
pub const LAYOUTS: [&Layout; 6] = [
    &crate::keymaps::us::US,
    &crate::keymaps::gb::GB,
    &crate::keymaps::de::DE,
    &crate::keymaps::fr::FR,
    &crate::keymaps::es::ES,
    &crate::keymaps::nordic::NORDIC,
];

impl Layout {
    /// The layout named `id`.
    pub fn by_id(id: &str) -> Option<&'static Layout> {
        LAYOUTS.iter().copied().find(|l| l.id == id)
    }

    /// What `usage` types with Shift `shift`, AltGr `altgr` and Caps Lock
    /// `caps`. Caps Lock turns a key only where Shift gives its capital (the
    /// letters, and ü or ñ, but not French é over 2), the other way from
    /// Shift; AltGr ignores both.
    pub fn sym(&self, usage: u16, shift: bool, altgr: bool, caps: bool) -> Sym {
        match usage {
            0x2C => return Sym::Char(' '),
            0x63 => return Sym::Char(self.keypad_decimal),
            0x54..=0x62 => return char_of(usage, false, false).map_or(Sym::None, Sym::Char),
            _ => {}
        }
        let Some(&(_, plain, shifted, alt)) = self.rows.iter().find(|r| r.0 == usage) else {
            return Sym::None;
        };
        if altgr {
            return alt;
        }
        let capital = match (plain, shifted) {
            (Sym::Char(p), Sym::Char(s)) => p.is_lowercase() && single_upper(p) == Some(s),
            _ => false,
        };
        let shift = if capital { shift != caps } else { shift };
        if shift { shifted } else { plain }
    }
}

/// `c` as its one uppercase character, if it has one.
fn single_upper(c: char) -> Option<char> {
    let mut upper = c.to_uppercase();
    let first = upper.next()?;
    upper.next().is_none().then_some(first)
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

/// Which Shift and Ctrl keys are held, a bit for each side's key: a key-down
/// sets its bit and a key-up clears it, so a held key's typematic repeats
/// (more downs than ups) cannot leave it held after its release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held(u8);

impl Held {
    /// `usage` going `down` or up: true when it is a Shift or Ctrl key, which
    /// this took, and false for any other key.
    pub fn key(&mut self, usage: u16, down: bool) -> bool {
        use crate::desk::usage as u;
        let bit = match usage {
            u::LEFT_SHIFT => 1,
            u::RIGHT_SHIFT => 2,
            u::LEFT_CTRL => 4,
            u::RIGHT_CTRL => 8,
            _ => return false,
        };
        match down {
            true => self.0 |= bit,
            false => self.0 &= !bit,
        }
        true
    }

    pub fn shift(self) -> bool {
        self.0 & 3 != 0
    }

    pub fn ctrl(self) -> bool {
        self.0 & 12 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_modifiers_repeats_end_with_its_one_release() {
        use crate::desk::usage as u;
        let mut held = Held::default();
        for _ in 0..3 {
            assert!(held.key(u::LEFT_SHIFT, true));
        }
        assert!(held.shift());
        held.key(u::LEFT_SHIFT, false);
        assert!(!held.shift(), "three makes and one break: not held");
        // Each side is its own key: the other still holds.
        held.key(u::LEFT_CTRL, true);
        held.key(u::RIGHT_CTRL, true);
        held.key(u::LEFT_CTRL, false);
        assert!(held.ctrl());
        held.key(u::RIGHT_CTRL, false);
        assert!(!held.ctrl());
        assert!(!held.key(0x04, true), "a letter is not taken");
    }

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
    fn each_accent_lists_as_many_letters_as_it_makes() {
        for (accent, spacing, from, to) in ACCENTS {
            assert_eq!(from.chars().count(), to.chars().count(), "{accent:?}");
            assert_eq!(accent.spacing(), spacing);
        }
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
