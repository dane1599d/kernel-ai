//! What the code view hands the app (`kai:code/code-input`) as what the editor's
//! screen takes (`editcore::screen`): a key's name, what it types and the
//! modifiers held, or a click's cell and modifiers.

use editcore::screen::{Input, Key};

/// `kai:code`'s modifier bits.
pub const SHIFT: u8 = 1;
pub const CTRL: u8 = 2;
pub const ALT: u8 = 4;
pub const SUPER: u8 = 8;

/// The key a `key-event` is, if the editor has a use for it.
pub fn key(name: &str, text: &str, mods: u8) -> Option<Key> {
    if mods & (ALT | SUPER) != 0 {
        return None;
    }
    let named = match name {
        "enter" => Some(Key::Enter),
        "backspace" => Some(Key::Backspace),
        "delete" => Some(Key::Delete),
        "tab" => Some(Key::Tab),
        "esc" => Some(Key::Esc),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" => Some(Key::PageUp),
        "pagedown" => Some(Key::PageDown),
        "space" if mods & CTRL == 0 => Some(Key::Char(' ')),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let mut letters = name.chars();
    if let (Some(letter), None, true) = (letters.next(), letters.next(), mods & CTRL != 0)
        && letter.is_ascii_lowercase()
    {
        return Some(Key::Ctrl(letter));
    }
    // Typed text is what the key types, with the shift already applied.
    let mut typed = text.chars();
    match (typed.next(), typed.next(), mods & CTRL) {
        (Some(c), None, 0) => Some(Key::Char(c)),
        _ => None,
    }
}

/// A key as the screen takes it.
pub fn key_input(name: &str, text: &str, mods: u8) -> Option<Input> {
    key(name, text, mods).map(Input::Key)
}

/// A click on text cell `cell` of row `row`, the gutter being `gutter` cells.
pub fn click_input(row: u32, cell: u32, mods: u8, gutter: usize) -> Input {
    Input::Click {
        row: row as usize,
        col: gutter + cell as usize,
        ctrl: mods & CTRL != 0,
    }
}
