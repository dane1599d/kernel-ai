//! ADR 0007's input event: 16 bytes, `time_ns: u64, kind: u8, _r: u8, code: u16
//! (HID usage), value: i32`, little-endian. The kernel's one input queue holds
//! these (INPUT_READ, P3), and `desktop` passes each on to the focused surface in
//! its header page's event ring (`surfabi::header`). Key codes are HID usage IDs
//! from the USB HID Usage Tables (page 0x07 for keys, 0x09 for buttons), never
//! Linux evdev numbers. [`Set1`] turns the i8042's scancode set 1, which USB
//! keyboards are also turned into today, into those usages.

/// What an [`InputEvent`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// A key, `code` its usage on page 0x07; `value` 1 down, 0 up, 2 repeat.
    Key = 1,
    /// Relative motion; `code` 0 for x, 1 for y; `value` in device units.
    Rel = 2,
    /// Absolute position; `code` 0 for x, 1 for y; `value` 0..=32767.
    Abs = 3,
    /// A button, `code` its usage on page 0x09 (1 left, 2 right, 3 middle);
    /// `value` 1 down, 0 up.
    Button = 4,
    /// A wheel; `code` 0 vertical, 1 horizontal; `value` in detents.
    Wheel = 5,
}

impl Kind {
    pub fn from_raw(raw: u8) -> Option<Kind> {
        match raw {
            1 => Some(Kind::Key),
            2 => Some(Kind::Rel),
            3 => Some(Kind::Abs),
            4 => Some(Kind::Button),
            5 => Some(Kind::Wheel),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub time_ns: u64,
    pub kind: Kind,
    pub code: u16,
    pub value: i32,
}

impl InputEvent {
    pub const SIZE: usize = 16;

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        out[0..8].copy_from_slice(&self.time_ns.to_le_bytes());
        out[8] = self.kind as u8;
        out[10..12].copy_from_slice(&self.code.to_le_bytes());
        out[12..16].copy_from_slice(&self.value.to_le_bytes());
        out
    }

    /// `None` for an unknown kind or a reserved byte that is not 0.
    pub fn decode(bytes: &[u8; Self::SIZE]) -> Option<InputEvent> {
        if bytes[9] != 0 {
            return None;
        }
        Some(InputEvent {
            time_ns: u64::from_le_bytes(bytes[0..8].try_into().ok()?),
            kind: Kind::from_raw(bytes[8])?,
            code: u16::from_le_bytes([bytes[10], bytes[11]]),
            value: i32::from_le_bytes(bytes[12..16].try_into().ok()?),
        })
    }
}

/// Scancode set 1 to HID keyboard usages (page 0x07), written from the USB HID
/// Usage Tables 1.5 (section 10) and the PC's published set 1 make codes. Index
/// by the make code; 0 means no key.
const SET1: [u8; 0x59] = [
    0x00, 0x29, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, // 00 -, Esc, 1-6
    0x24, 0x25, 0x26, 0x27, 0x2D, 0x2E, 0x2A, 0x2B, // 08 7-0, -, =, Backspace, Tab
    0x14, 0x1A, 0x08, 0x15, 0x17, 0x1C, 0x18, 0x0C, // 10 Q W E R T Y U I
    0x12, 0x13, 0x2F, 0x30, 0x28, 0xE0, 0x04, 0x16, // 18 O P [ ] Enter LCtrl A S
    0x07, 0x09, 0x0A, 0x0B, 0x0D, 0x0E, 0x0F, 0x33, // 20 D F G H J K L ;
    0x34, 0x35, 0xE1, 0x31, 0x1D, 0x1B, 0x06, 0x19, // 28 ' ` LShift \ Z X C V
    0x05, 0x11, 0x10, 0x36, 0x37, 0x38, 0xE5, 0x55, // 30 B N M , . / RShift KP*
    0xE2, 0x2C, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, // 38 LAlt Space Caps F1-F5
    0x3F, 0x40, 0x41, 0x42, 0x43, 0x53, 0x47, 0x5F, // 40 F6-F10 NumLock ScrollLock KP7
    0x60, 0x61, 0x56, 0x5C, 0x5D, 0x5E, 0x57, 0x59, // 48 KP8 KP9 KP- KP4 KP5 KP6 KP+ KP1
    0x5A, 0x5B, 0x62, 0x63, 0x00, 0x00, 0x64, 0x44, // 50 KP2 KP3 KP0 KP. - - NonUS\ F11
    0x45, // 58 F12
];

/// The `E0`-prefixed make codes that are keys, and their usages.
const SET1_E0: [(u8, u8); 17] = [
    (0x1C, 0x58), // keypad Enter
    (0x1D, 0xE4), // right Ctrl
    (0x35, 0x54), // keypad /
    (0x38, 0xE6), // right Alt
    (0x47, 0x4A), // Home
    (0x48, 0x52), // Up
    (0x49, 0x4B), // Page Up
    (0x4B, 0x50), // Left
    (0x4D, 0x4F), // Right
    (0x4F, 0x4D), // End
    (0x50, 0x51), // Down
    (0x51, 0x4E), // Page Down
    (0x52, 0x49), // Insert
    (0x53, 0x4C), // Delete
    (0x5B, 0xE3), // left GUI (Super)
    (0x5C, 0xE7), // right GUI
    (0x5D, 0x65), // Application
];

/// Usages of the modifiers [`Set1`] tracks.
pub mod usage {
    pub const LEFT_CTRL: u16 = 0xE0;
    pub const LEFT_SHIFT: u16 = 0xE1;
    pub const LEFT_ALT: u16 = 0xE2;
    pub const LEFT_GUI: u16 = 0xE3;
    pub const RIGHT_CTRL: u16 = 0xE4;
    pub const RIGHT_SHIFT: u16 = 0xE5;
    pub const RIGHT_ALT: u16 = 0xE6;
    pub const RIGHT_GUI: u16 = 0xE7;
    pub const F12: u16 = 0x45;
}

/// Decodes scancode set 1, a byte at a time, into (usage, down). `E0` prefixes
/// a second byte; `E1` (Pause) starts a six-byte sequence that is skipped, as
/// are `E0 2A` and `E0 B6`-style fake shifts. Ctrl and Alt are tracked, so the
/// kernel can see Ctrl+Alt+F12.
#[derive(Clone, Copy, Debug, Default)]
pub struct Set1 {
    extended: bool,
    skip: u8,
    ctrl: u8,
    alt: u8,
}

impl Set1 {
    pub const fn new() -> Self {
        Self {
            extended: false,
            skip: 0,
            ctrl: 0,
            alt: 0,
        }
    }

    pub fn feed(&mut self, byte: u8) -> Option<(u16, bool)> {
        if self.skip > 0 {
            self.skip -= 1;
            return None;
        }
        match byte {
            0xE0 => {
                self.extended = true;
                return None;
            }
            0xE1 => {
                self.skip = 5;
                return None;
            }
            _ => {}
        }
        let extended = core::mem::take(&mut self.extended);
        let (make, down) = (byte & 0x7F, byte & 0x80 == 0);
        let usage = match extended {
            false => u16::from(*SET1.get(usize::from(make))?),
            true => u16::from(SET1_E0.iter().find(|(m, _)| *m == make)?.1),
        };
        if usage == 0 {
            return None;
        }
        let bit = |held: &mut u8, n: u8| match down {
            true => *held |= n,
            false => *held &= !n,
        };
        match usage {
            usage::LEFT_CTRL => bit(&mut self.ctrl, 1),
            usage::RIGHT_CTRL => bit(&mut self.ctrl, 2),
            usage::LEFT_ALT => bit(&mut self.alt, 1),
            usage::RIGHT_ALT => bit(&mut self.alt, 2),
            _ => {}
        }
        Some((usage, down))
    }

    /// Whether a Ctrl and an Alt key are both held.
    pub fn ctrl_alt(&self) -> bool {
        self.ctrl != 0 && self.alt != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(bytes: &[u8]) -> alloc::vec::Vec<(u16, bool)> {
        let mut set1 = Set1::new();
        bytes.iter().filter_map(|&b| set1.feed(b)).collect()
    }

    #[test]
    fn set1_makes_and_breaks_turn_into_hid_usages() {
        // a, Enter, Esc, F12 down and up; keypad Enter and Up behind E0.
        assert_eq!(feed(&[0x1E, 0x9E]), [(0x04, true), (0x04, false)]);
        assert_eq!(
            feed(&[0x1C, 0x01, 0x58, 0xD8]),
            [(0x28, true), (0x29, true), (0x45, true), (0x45, false)]
        );
        assert_eq!(
            feed(&[0xE0, 0x1C, 0xE0, 0x48, 0xE0, 0xC8]),
            [(0x58, true), (0x52, true), (0x52, false)]
        );
        assert_eq!(feed(&[0xE0, 0x5B]), [(usage::LEFT_GUI, true)]);
        // Pause's E1 sequence and unknown codes give nothing.
        assert_eq!(feed(&[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5, 0x54, 0x7F]), []);
    }

    #[test]
    fn every_set1_usage_is_one_key() {
        let mut seen = alloc::vec::Vec::new();
        for usage in SET1.iter().chain(SET1_E0.iter().map(|(_, u)| u)) {
            if *usage != 0 {
                assert!(!seen.contains(usage), "{usage:#x} twice");
                seen.push(*usage);
            }
        }
        assert_eq!(seen.len(), 86 + 17);
    }

    #[test]
    fn ctrl_alt_is_tracked_across_left_and_right() {
        let mut set1 = Set1::new();
        for b in [0x1D, 0xE0, 0x38] {
            set1.feed(b);
        }
        assert!(set1.ctrl_alt());
        set1.feed(0x9D);
        assert!(!set1.ctrl_alt());
        set1.feed(0xE0);
        set1.feed(0x1D);
        assert!(set1.ctrl_alt());
    }
}
