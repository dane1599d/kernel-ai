//! What a screen is told. The kernel's console, `uikit` and a test each have
//! their own key type; this is the one the model reads, so it depends on none.

/// A key, as `termcore::keys::Key` names them.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Key(Key),
    /// A click on the text area: row and column in cells from its top left,
    /// the gutter included, and whether Ctrl was held.
    Click {
        row: usize,
        col: usize,
        ctrl: bool,
    },
}
