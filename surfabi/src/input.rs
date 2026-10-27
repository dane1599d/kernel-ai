//! ADR 0007's input event: 16 bytes, `time_ns: u64, kind: u8, _r: u8, code: u16
//! (HID usage), value: i32`, little-endian. The kernel's one input queue holds
//! these (INPUT_READ), and `desktop` passes each on to the focused surface in its
//! header page's event ring. Key codes are HID usage IDs from the USB HID Usage
//! Tables (page 0x07 for keys, 0x09 for buttons), never Linux evdev numbers.

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
