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
    /// A key, `code` its usage on page 0x07; `value` 1 down, 0 up, 2 repeat,
    /// or [`TAP`]: a Super tapped alone, which the kernel saw whole (it ends
    /// the lease if a terminal held the keys on one). No other Super event
    /// opens Start.
    Key = 1,
    /// Relative motion; `code` 0 for x, 1 for y; `value` in device units.
    Rel = 2,
    /// Absolute position; `code` 0 for x, 1 for y; `value` 0..=32767.
    Abs = 3,
    /// A button, `code` its usage on page 0x09 (1 left, 2 right, 3 middle);
    /// `value` 1 down, 0 up.
    Button = 4,
    /// A wheel; `code` 0 vertical, 1 horizontal; `value` in detents, positive
    /// toward the person (scroll down) or to the right. A PS/2 IntelliMouse's and
    /// a USB HID wheel both give `code` 0 (backlog item 10).
    Wheel = 5,
    /// Ours, not ADR 0007's: the screen changed hands. `value` 0 when
    /// Ctrl+Alt+F12 gave it to the console (the claim is revoked), 1 when a
    /// second press gives it back (claim it again). Added 2026-11-25, with
    /// no ABI bump: [`INPUT_LOST`] (2), events were dropped before this one.
    /// A reader acts only on the values it knows and passes over any other,
    /// as every reader in the tree does.
    Screen = 6,
}

impl Kind {
    pub fn from_raw(raw: u8) -> Option<Kind> {
        match raw {
            1 => Some(Kind::Key),
            2 => Some(Kind::Rel),
            3 => Some(Kind::Abs),
            4 => Some(Kind::Button),
            5 => Some(Kind::Wheel),
            6 => Some(Kind::Screen),
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
    pub const TAB: u16 = 0x2B;
    pub const ESCAPE: u16 = 0x29;
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
    /// The Super keys held: bit 0 left, bit 1 right.
    gui: u8,
    /// A Super key went down and no other key since: its release is a tap.
    super_alone: bool,
    /// The last key decoded was a Super tap's release.
    super_tap: bool,
    /// Every key held, by make code: bits 0-127 plain, 128-255 after `E0`.
    /// Unlike the console's decoder, it is never started afresh when the
    /// keys change hands.
    down: [u64; 4],
    /// Ctrl+Alt+F12 went down and nothing was decoded since but its repeats
    /// (typematic repeats only the last key pressed): [`Set1::switch_press`].
    switch_down: bool,
}

impl Set1 {
    pub const fn new() -> Self {
        Self {
            extended: false,
            skip: 0,
            ctrl: 0,
            alt: 0,
            gui: 0,
            super_alone: false,
            super_tap: false,
            down: [0; 4],
            switch_down: false,
        }
    }

    /// The keys held now, as the scancode bytes that pressed them: a make code,
    /// and whether `E0` came first.
    pub fn held_makes(&self) -> impl Iterator<Item = (bool, u8)> + '_ {
        (0..256usize)
            .filter(|&n| self.down[n / 64] >> (n % 64) & 1 != 0)
            .map(|n| (n >= 128, (n % 128) as u8))
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
                // Pause: a key pressed, though it decodes to none.
                self.skip = 5;
                self.super_alone = false;
                self.switch_down = false;
                return None;
            }
            _ => {}
        }
        let extended = core::mem::take(&mut self.extended);
        let (make, down) = (byte & 0x7F, byte & 0x80 == 0);
        let slot = usize::from(extended) * 128 + usize::from(make);
        match down {
            true => self.down[slot / 64] |= 1 << (slot % 64),
            false => self.down[slot / 64] &= !(1 << (slot % 64)),
        }
        let usage = match extended {
            false => SET1.get(usize::from(make)).map(|u| u16::from(*u)),
            true => SET1_E0
                .iter()
                .find(|(m, _)| *m == make)
                .map(|(_, u)| u16::from(*u)),
        }
        .unwrap_or(0);
        // F12's own release ends a press of it, and so does another key's
        // make (typematic repeats only the last key pressed, so after one the
        // F12 make is a new press): a lost break is forgiven once the user
        // presses another key, Ctrl or Alt again, say. Another key's release
        // does not: Shift let go while F12 is held leaves its repeats repeats.
        if usage == usage::F12 && !down || usage != usage::F12 && down {
            self.switch_down = false;
        }
        if usage == 0 {
            // A key with no usage here (PrintScreen, media keys, a fake
            // shift, `UNMAPPED`): pressed with Super, Super was not tapped.
            if down {
                self.super_alone = false;
            }
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
        let gui = matches!(usage, usage::LEFT_GUI | usage::RIGHT_GUI);
        // A Super's first make, per key: its typematic repeats are makes too,
        // and a lost break of the other Super must not stop this one's taps.
        let n = match usage {
            usage::LEFT_GUI => 1,
            usage::RIGHT_GUI => 2,
            _ => 0,
        };
        let first = down && n != 0 && self.gui & n == 0;
        if n != 0 {
            bit(&mut self.gui, n);
        }
        self.super_tap = gui && !down && self.super_alone;
        self.super_alone = match (gui, down) {
            // Only the first make starts a tap: a repeat after a click
            // (`break_tap`) or another key must not start it again.
            (true, true) => first || self.super_alone,
            (true, false) => false,
            // Any other key pressed with Super held: not a tap.
            (false, true) => false,
            (false, false) => self.super_alone,
        };
        Some((usage, down))
    }

    /// Something that is not a key happened with Super held (a pointer
    /// button pressed, the screen changing hands): Super's release is no tap.
    /// The kernel applies it before the next key byte it routes, so it must
    /// see a device's key bytes in the order they and the press happened
    /// (the USB poll injects a report's earlier keys before a pointer report).
    pub fn break_tap(&mut self) {
        self.super_alone = false;
    }

    /// Whether the Ctrl+Alt+F12 make just routed ([`KeyRoute::Screen`]) is a
    /// new press, not a typematic repeat of one: true once per press. A held
    /// key repeats its make, and a repeat must not switch the screen again.
    pub fn switch_press(&mut self, down: bool) -> bool {
        let first = down && !self.switch_down;
        self.switch_down |= down;
        first
    }

    /// Whether a Ctrl and an Alt key are both held.
    pub fn ctrl_alt(&self) -> bool {
        self.ctrl != 0 && self.alt != 0
    }

    /// Whether an Alt key is held.
    pub fn alt(&self) -> bool {
        self.alt != 0
    }

    /// Where the key just decoded, `usage` going `down` or up, goes, with a
    /// terminal holding the keys on a lease (`leased`) or not. Call it after
    /// [`Set1::feed`] gave the key. Under a lease, `desktop` gets only what
    /// it must to switch away: its own chords, which end the lease (Alt+Tab,
    /// Ctrl+Esc's press, a Super tapped alone), and Ctrl and Alt, so a chord
    /// reaches it whole. Super is not shared: a Super used with another key
    /// is the terminal's alone, so `desktop` never takes it for a tap.
    ///
    /// Only this side sees every key (PrintScreen, Pause and unmapped USB
    /// keys decode to none), so a Super tap is [`KeyRoute::Tap`] with or
    /// without a lease; `desktop` takes no other Super event for one.
    pub fn route(&self, usage: u16, down: bool, leased: bool) -> KeyRoute {
        if usage == usage::F12 && self.ctrl_alt() {
            return KeyRoute::Screen;
        }
        let tap = matches!(usage, usage::LEFT_GUI | usage::RIGHT_GUI) && !down && self.super_tap;
        if !leased {
            return match tap {
                true => KeyRoute::Tap,
                false => KeyRoute::Desk,
            };
        }
        // Chords on the press only: a release rolled over a modifier is not.
        let chord = match usage {
            usage::TAB => down && self.alt != 0,
            usage::ESCAPE => down && self.ctrl != 0,
            _ => false,
        };
        let shared = matches!(
            usage,
            usage::LEFT_CTRL | usage::RIGHT_CTRL | usage::LEFT_ALT | usage::RIGHT_ALT
        );
        match (chord, tap, shared) {
            (true, _, _) => KeyRoute::Chord,
            (_, true, _) => KeyRoute::Tap,
            (_, _, true) => KeyRoute::Both,
            _ => KeyRoute::Console,
        }
    }

    /// Whether the last byte ended its sequence: no `E0` or `E1` is waiting
    /// for the rest, so the bytes after it may go elsewhere.
    pub fn at_boundary(&self) -> bool {
        !self.extended && self.skip == 0
    }
}

/// [`Kind::Key`]'s value for a Super tap the kernel saw: the only Super
/// event that opens Start.
pub const TAP: i32 = 3;

/// [`Kind::Screen`]'s value when the kernel's queue was full and dropped
/// its oldest events since the last INPUT_READ: the read starts with it,
/// once. A button's or a key's release may be among those dropped, so
/// `desktop` ends what one held (a press in a window, a move).
pub const INPUT_LOST: i32 = 2;

/// The set 1 make code a USB keyboard's key with no set 1 code is sent as
/// (its break with bit 7): it decodes to no usage, so it reaches no one, but
/// it is a key pressed, so Super held with it is not a tap.
pub const UNMAPPED: u8 = 0x54;

/// Who gets the keys: the console (the screen is its, or a lease lends them)
/// or `desktop`. Changed only at a sequence boundary ([`Set1::at_boundary`]).
/// Whenever it changes, either way, the side that loses the keys never sees
/// the releases of keys it saw go down, and the side that gets them never saw
/// those go down: so the console's ring is emptied and its key decoder starts
/// afresh (`kernel/src/input`), or a Ctrl held across Ctrl+Alt+F12 stays held
/// in the console for good (bug hunt 2, H2-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hands {
    console: bool,
}

impl Hands {
    /// The console holds the screen at boot, and its keys.
    pub const fn new() -> Self {
        Self { console: true }
    }

    /// At a sequence boundary: `desktop` holds the screen (`taking`), and a
    /// lease lends the keys to the console (`leased`, which counts only while
    /// `desktop` holds the screen). True if the keys changed hands.
    pub fn settle(&mut self, taking: bool, leased: bool) -> bool {
        let console = !taking || leased;
        let changed = console != self.console;
        self.console = console;
        changed
    }

    /// Whether keys go to the console now.
    pub fn console(&self) -> bool {
        self.console
    }
}

impl Default for Hands {
    /// As [`Hands::new`]: the console holds the keys at boot, not neither.
    fn default() -> Self {
        Self::new()
    }
}

/// Where a key goes ([`Set1::route`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRoute {
    /// To `desktop`'s queue: no lease holds the keys.
    Desk,
    /// To the console's terminal alone.
    Console,
    /// To both: Ctrl or Alt under a lease, so a chord reaches `desktop`.
    Both,
    /// `desktop`'s chord (Alt+Tab, Ctrl+Esc): the lease ends and it goes to
    /// `desktop` alone.
    Chord,
    /// A Super tapped alone: a lease, if any, ends and `desktop` gets one
    /// [`TAP`] event.
    Tap,
    /// Ctrl+Alt+F12: the screen changes hands; nobody gets the key.
    Screen,
}

#[cfg(test)]
mod tests;
