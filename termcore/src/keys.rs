//! Keys from a PS/2 keyboard (scan code set 1, US layout), and the tokens that name
//! them on the edit service's `keys` request.
//!
//! Decoding is pc-keyboard's, with Ctrl mapping letters to control codes: a letter
//! typed with either Ctrl held is a Ctrl-letter, while Backspace, Tab and Enter stay
//! themselves under Ctrl. NumLock starts on, so the numpad types digits until it is
//! pressed.
//!
//! Tokens follow each other with no separator. Printable ASCII stands for itself,
//! except `<`, written `<lt>`. Other keys are `<up>`, `<down>`, `<left>`, `<right>`,
//! `<home>`, `<end>`, `<pgup>`, `<pgdn>`, `<del>`, `<bs>`, `<tab>`, `<enter>` and
//! `<esc>`, and Ctrl-letters `<C-a>` to `<C-z>`. Ctrl-click will add `<click r c>`
//! and `<C-click r c>`, counting rows and columns from 0 like `screen: cursor`.

use alloc::string::String;
use alloc::vec::Vec;
use pc_keyboard::layouts::Us104Key;
use pc_keyboard::{DecodedKey, HandleControl, KeyCode, KeyState, PS2Keyboard, ScancodeSet1};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable character.
    Char(char),
    Enter,
    Backspace,
    Delete,
    Tab,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// A letter, `'a'` to `'z'`, typed with Ctrl held.
    Ctrl(char),
}

/// Keys written `<name>`.
const NAMED: [Key; 13] = [
    Key::Up,
    Key::Down,
    Key::Left,
    Key::Right,
    Key::Home,
    Key::End,
    Key::PageUp,
    Key::PageDown,
    Key::Delete,
    Key::Backspace,
    Key::Tab,
    Key::Enter,
    Key::Esc,
];

impl Key {
    /// Appends the key's token to `out`.
    pub fn push_token(&self, out: &mut String) {
        match (*self, self.name()) {
            (Key::Char('<'), _) => out.push_str("<lt>"),
            (Key::Char(ch), _) => out.push(ch),
            (Key::Ctrl(letter), _) => {
                out.push_str("<C-");
                out.push(letter);
                out.push('>');
            }
            (_, name) => {
                out.push('<');
                out.push_str(name.unwrap_or("?"));
                out.push('>');
            }
        }
    }

    /// The name in a `<name>` token, for the keys that have one. Every variant is
    /// listed, so a new key cannot be left without a token.
    fn name(self) -> Option<&'static str> {
        Some(match self {
            Key::Char(_) | Key::Ctrl(_) => return None,
            Key::Up => "up",
            Key::Down => "down",
            Key::Left => "left",
            Key::Right => "right",
            Key::Home => "home",
            Key::End => "end",
            Key::PageUp => "pgup",
            Key::PageDown => "pgdn",
            Key::Delete => "del",
            Key::Backspace => "bs",
            Key::Tab => "tab",
            Key::Enter => "enter",
            Key::Esc => "esc",
        })
    }
}

/// Why a token string was refused; offsets are in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenError {
    /// A control character or one outside ASCII.
    NotPrintable(usize),
    /// A `<` with no `>` after it.
    Unterminated(usize),
    /// A `<name>` that names no key.
    Unknown(usize),
}

/// The keys a token string names.
pub fn parse_tokens(text: &str) -> Result<Vec<Key>, TokenError> {
    if let Some((at, _)) = text
        .char_indices()
        .find(|&(_, ch)| !(' '..='~').contains(&ch))
    {
        return Err(TokenError::NotPrintable(at));
    }
    let mut keys = Vec::new();
    let mut at = 0;
    while let Some(&byte) = text.as_bytes().get(at) {
        if byte != b'<' {
            keys.push(Key::Char(char::from(byte)));
            at += 1;
            continue;
        }
        let end = text[at..].find('>').ok_or(TokenError::Unterminated(at))? + at;
        keys.push(named(&text[at + 1..end]).ok_or(TokenError::Unknown(at))?);
        at = end + 1;
    }
    Ok(keys)
}

fn named(name: &str) -> Option<Key> {
    if name == "lt" {
        return Some(Key::Char('<'));
    }
    if let Some(letter) = name.strip_prefix("C-") {
        return match letter.as_bytes() {
            &[letter @ b'a'..=b'z'] => Some(Key::Ctrl(char::from(letter))),
            _ => None,
        };
    }
    NAMED.into_iter().find(|key| key.name() == Some(name))
}

/// Keys [`Decoder`] tracks as held at once; more are still decoded, just not
/// tracked (a repeat of an untracked key reads as a fresh press).
const HELD: usize = 8;

/// A physical key, as [`Stroke`] names it: what [`Decoder::held`] lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Code(KeyCode);

/// One key going down or up, from [`Decoder::stroke`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stroke {
    pub code: Code,
    /// The key it types, for a press; `None` for a release, a modifier or a key
    /// with no meaning here.
    pub key: Option<Key>,
    pub down: bool,
    /// A press of a key already held: the keyboard's auto-repeat.
    pub repeat: bool,
}

/// Turns keyboard bytes into keys, keeping Shift, Ctrl and NumLock state, and
/// which keys are held.
pub struct Decoder {
    keyboard: PS2Keyboard<Us104Key, ScancodeSet1>,
    held: [Option<KeyCode>; HELD],
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub const fn new() -> Self {
        Self {
            keyboard: PS2Keyboard::new(
                ScancodeSet1::new(),
                Us104Key,
                HandleControl::MapLettersToUnicode,
            ),
            held: [None; HELD],
        }
    }

    /// Takes one byte from the keyboard. None for break codes, prefixes, modifier
    /// keys, keys with no meaning here, and bytes that do not decode.
    pub fn feed(&mut self, byte: u8) -> Option<Key> {
        self.stroke(byte).and_then(|stroke| stroke.key)
    }

    /// Takes one byte from the keyboard: the stroke it ends, if any. None for
    /// prefixes and bytes that do not decode.
    pub fn stroke(&mut self, byte: u8) -> Option<Stroke> {
        let event = self.keyboard.add_byte(byte).ok()??;
        let code = event.code;
        let down = event.state != KeyState::Up;
        let was_held = self.held.contains(&Some(code));
        match (down, was_held) {
            (true, false) => {
                if let Some(free) = self.held.iter_mut().find(|h| h.is_none()) {
                    *free = Some(code);
                }
            }
            (false, _) => {
                for held in self.held.iter_mut().filter(|h| **h == Some(code)) {
                    *held = None;
                }
            }
            (true, true) => {}
        }
        // Every event goes through, so the modifiers stay right.
        let key = match self.keyboard.process_keyevent(event) {
            Some(DecodedKey::Unicode(ch)) => self.char_key(ch, code),
            Some(DecodedKey::RawKey(code)) => code_key(code),
            None => None,
        };
        Some(Stroke {
            code: Code(code),
            key: key.filter(|_| down),
            down,
            repeat: down && was_held,
        })
    }

    /// The keys held now, as far as it tracks them.
    pub fn held(&self) -> impl Iterator<Item = Code> + '_ {
        self.held.iter().flatten().map(|&code| Code(code))
    }

    /// Whether either Ctrl is held.
    pub fn ctrl(&self) -> bool {
        self.keyboard.get_modifiers().is_ctrl()
    }

    fn char_key(&self, ch: char, code: KeyCode) -> Option<Key> {
        // Ctrl-H, I and J are 0x08 to 0x0A like Backspace, Tab and Enter, which
        // stay themselves under Ctrl.
        let itself = matches!(
            code,
            KeyCode::Backspace | KeyCode::Tab | KeyCode::Return | KeyCode::NumpadEnter
        );
        if ('\u{1}'..='\u{1a}').contains(&ch) && self.ctrl() && !itself {
            return Some(Key::Ctrl(char::from(ch as u8 + 0x60)));
        }
        match ch {
            '\n' => Some(Key::Enter),
            '\u{8}' => Some(Key::Backspace),
            '\t' => Some(Key::Tab),
            '\u{1b}' => Some(Key::Esc),
            '\u{7f}' => Some(Key::Delete),
            ch if !ch.is_control() => Some(Key::Char(ch)),
            _ => None,
        }
    }
}

fn code_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::ArrowUp => Key::Up,
        KeyCode::ArrowDown => Key::Down,
        KeyCode::ArrowLeft => Key::Left,
        KeyCode::ArrowRight => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use Key::{Char, Ctrl};
    use alloc::vec;

    fn keys(bytes: &[u8]) -> Vec<Key> {
        let mut decoder = Decoder::new();
        bytes
            .iter()
            .filter_map(|&byte| decoder.feed(byte))
            .collect()
    }

    #[test]
    fn extended_keys_decode_from_real_set_1_bytes() {
        let cases = [
            (0x48, Key::Up),
            (0x50, Key::Down),
            (0x4B, Key::Left),
            (0x4D, Key::Right),
            (0x47, Key::Home),
            (0x4F, Key::End),
            (0x49, Key::PageUp),
            (0x51, Key::PageDown),
            (0x53, Key::Delete),
        ];
        for (code, key) in cases {
            assert_eq!(
                keys(&[0xE0, code, 0xE0, code | 0x80]),
                [key],
                "E0 {code:02X}"
            );
        }
    }

    #[test]
    fn strokes_say_what_is_held_and_what_repeats() {
        let mut decoder = Decoder::new();
        let strokes: Vec<Stroke> = [0x15, 0x15, 0x2A, 0x95, 0xAA]
            .iter()
            .filter_map(|&byte| decoder.stroke(byte))
            .collect();
        let shown: Vec<(Option<Key>, bool, bool)> =
            strokes.iter().map(|s| (s.key, s.down, s.repeat)).collect();
        assert_eq!(
            shown,
            [
                (Some(Char('y')), true, false),
                (Some(Char('y')), true, true),
                (None, true, false),
                (None, false, false),
                (None, false, false),
            ]
        );
        assert_eq!(decoder.held().count(), 0);
        let enter = decoder.stroke(0x1C).unwrap();
        assert_eq!(decoder.held().collect::<Vec<_>>(), [enter.code]);
        assert_ne!(enter.code, strokes[0].code);
    }

    #[test]
    fn esc_backspace_tab_and_enter() {
        assert_eq!(keys(&[0x01, 0x81]), [Key::Esc]);
        assert_eq!(keys(&[0x0E, 0x8E]), [Key::Backspace]);
        assert_eq!(keys(&[0x0F, 0x8F]), [Key::Tab]);
        assert_eq!(keys(&[0x1C, 0x9C]), [Key::Enter]);
    }

    #[test]
    fn a_letter_with_ctrl_held_is_a_ctrl_letter() {
        let mut decoder = Decoder::new();
        assert_eq!(decoder.feed(0x1D), None);
        assert!(decoder.ctrl());
        assert_eq!(decoder.feed(0x2E), Some(Ctrl('c')));
        assert_eq!(decoder.feed(0xAE), None);
        assert_eq!(decoder.feed(0x9D), None);
        assert!(!decoder.ctrl());
        assert_eq!(decoder.feed(0x2E), Some(Char('c')));
        assert_eq!(keys(&[0xE0, 0x1D, 0x30, 0xB0, 0xE0, 0x9D]), [Ctrl('b')]);
    }

    #[test]
    fn backspace_tab_and_enter_stay_themselves_under_ctrl() {
        assert_eq!(keys(&[0x1D, 0x0E, 0x8E, 0x9D]), [Key::Backspace]);
        assert_eq!(keys(&[0x1D, 0x0F, 0x8F, 0x9D]), [Key::Tab]);
        assert_eq!(keys(&[0x1D, 0x1C, 0x9C, 0x9D]), [Key::Enter]);
        // Ctrl-H, I, J and M share their codes with those keys but are letters.
        assert_eq!(
            keys(&[0x1D, 0x23, 0x17, 0x24, 0x32, 0x9D]),
            [Ctrl('h'), Ctrl('i'), Ctrl('j'), Ctrl('m')]
        );
    }

    #[test]
    fn shift_types_capitals() {
        assert_eq!(keys(&[0x2A, 0x1E, 0x9E, 0xAA]), [Char('A')]);
    }

    #[test]
    fn a_fake_shift_around_home_gives_home_only() {
        assert_eq!(
            keys(&[0xE0, 0x2A, 0xE0, 0x47, 0xE0, 0xC7, 0xE0, 0xAA]),
            [Key::Home]
        );
    }

    #[test]
    fn the_numpad_types_digits_until_numlock_is_pressed() {
        assert_eq!(keys(&[0x48, 0xC8]), [Char('8')]);
        assert_eq!(keys(&[0x45, 0xC5, 0x48, 0xC8]), [Key::Up]);
    }

    #[test]
    fn noise_gives_nothing_and_the_next_key_still_decodes() {
        // A lone break code, Shift and Ctrl tapped, F1, Insert, unknown codes, then a.
        let noise = [0x9E, 0x2A, 0xAA, 0x1D, 0x9D, 0x3B, 0xBB, 0xE0, 0x52];
        let unknown = [0x7F, 0xE0, 0x7F, 0x1E];
        assert_eq!(keys(&[&noise[..], &unknown[..]].concat()), [Char('a')]);
    }

    #[test]
    fn every_key_round_trips_through_its_token() {
        let mut all = vec![Char(' '), Char('<'), Char('>'), Char('~'), Char('a')];
        all.extend([Ctrl('a'), Ctrl('z')]);
        all.extend(NAMED);
        let mut text = String::new();
        for key in &all {
            key.push_token(&mut text);
        }
        assert_eq!(parse_tokens(&text), Ok(all));
    }

    #[test]
    fn tokens_follow_each_other_without_separators() {
        assert_eq!(
            parse_tokens("ab <lt><up><C-b>"),
            Ok(vec![
                Char('a'),
                Char('b'),
                Char(' '),
                Char('<'),
                Key::Up,
                Ctrl('b')
            ])
        );
    }

    #[test]
    fn bad_tokens_are_refused() {
        assert_eq!(parse_tokens("x<nope>"), Err(TokenError::Unknown(1)));
        assert_eq!(parse_tokens("<C-1>"), Err(TokenError::Unknown(0)));
        assert_eq!(parse_tokens("<C-ab>"), Err(TokenError::Unknown(0)));
        assert_eq!(parse_tokens("ab<up"), Err(TokenError::Unterminated(2)));
        assert_eq!(parse_tokens("a\tb"), Err(TokenError::NotPrintable(1)));
        assert_eq!(parse_tokens("\u{e9}"), Err(TokenError::NotPrintable(0)));
    }
}
