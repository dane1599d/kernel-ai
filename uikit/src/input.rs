//! Input as `uikit` takes it: one window's events from `kai-surface 1`'s event
//! ring, positions already in window-local logical px and keys as HID usages
//! with their text (ADR 0005 section 3), so apps need no keymap.

use crate::geom::Point;
use alloc::string::String;

/// The keys widgets act on; anything else is `Other` with its HID usage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Backspace,
    Tab,
    Space,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Other(u16),
}

impl Key {
    /// From a HID usage on the keyboard page (0x07), USB HID Usage Tables 1.5 §10.
    pub const fn from_hid(usage: u16) -> Self {
        match usage {
            0x28 | 0x58 => Self::Enter,
            0x29 => Self::Escape,
            0x2A => Self::Backspace,
            0x2B => Self::Tab,
            0x2C => Self::Space,
            0x4A => Self::Home,
            0x4B => Self::PageUp,
            0x4C => Self::Delete,
            0x4D => Self::End,
            0x4E => Self::PageDown,
            0x4F => Self::Right,
            0x50 => Self::Left,
            0x51 => Self::Down,
            0x52 => Self::Up,
            other => Self::Other(other),
        }
    }
}

/// Modifier keys held, as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods(pub u8);

impl Mods {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
    pub const SUPER: u8 = 8;

    pub const fn shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }

    /// Ctrl, Alt or Super: the key is a shortcut, not text.
    pub const fn command(self) -> bool {
        self.0 & (Self::CTRL | Self::ALT | Self::SUPER) != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    PointerMove(Point),
    Button {
        at: Point,
        down: bool,
    },
    /// Wheel steps, positive down.
    Wheel {
        at: Point,
        dy: i32,
    },
    Key {
        key: Key,
        down: bool,
        mods: Mods,
        text: String,
    },
    /// `desktop` gave the window keyboard focus, or took it away.
    FocusIn,
    FocusOut,
}
