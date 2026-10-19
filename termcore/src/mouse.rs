//! PS/2 mouse packets and the pointer they move.
//!
//! A PS/2 mouse in stream mode sends 3-byte packets: flags, X delta, Y delta. The
//! flags byte always has bit 3 set and carries the buttons in bits 0 to 2, the
//! deltas' sign bits in bits 4 and 5 and their overflow bits in bits 6 and 7. PS/2
//! Y grows upward; screen Y grows downward.

const ALWAYS_ONE: u8 = 1 << 3;
const X_SIGN: u8 = 1 << 4;
const Y_SIGN: u8 = 1 << 5;
const X_OVERFLOW: u8 = 1 << 6;
const Y_OVERFLOW: u8 = 1 << 7;
const BUTTON_BITS: u8 = 0b111;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    pub const ALL: [Button; 3] = [Button::Left, Button::Middle, Button::Right];

    /// The usual button number: 1 left, 2 middle, 3 right.
    pub const fn number(self) -> u8 {
        match self {
            Button::Left => 1,
            Button::Middle => 2,
            Button::Right => 3,
        }
    }

    const fn bit(self) -> u8 {
        match self {
            Button::Left => 1,
            Button::Right => 1 << 1,
            Button::Middle => 1 << 2,
        }
    }
}

/// The buttons held, as a packet's flags carry them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buttons(u8);

impl Buttons {
    pub const NONE: Buttons = Buttons(0);

    pub const fn held(self, button: Button) -> bool {
        self.0 & button.bit() != 0
    }

    /// The buttons held now that were not held `before`, left first.
    pub fn pressed_since(self, before: Buttons) -> impl Iterator<Item = Button> {
        Button::ALL
            .into_iter()
            .filter(move |&button| self.held(button) && !before.held(button))
    }
}

/// One packet's motion and buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Rightward motion, in counts.
    pub dx: i16,
    /// Upward motion, in counts, as PS/2 reports it.
    pub dy: i16,
    pub buttons: Buttons,
}

/// Turns the mouse's bytes into packets.
#[derive(Debug, Default)]
pub struct Decoder {
    bytes: [u8; 3],
    len: usize,
}

impl Decoder {
    pub const fn new() -> Self {
        Self {
            bytes: [0; 3],
            len: 0,
        }
    }

    /// Adds one byte and returns the packet it completes, if any. A first byte
    /// without bit 3 cannot start a packet and is dropped, so a stream that lost a
    /// byte finds its way back to a packet start. A packet with either overflow bit
    /// is dropped whole: its deltas are not the motion.
    pub fn push(&mut self, byte: u8) -> Option<Packet> {
        if self.len == 0 && byte & ALWAYS_ONE == 0 {
            return None;
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        if self.len < self.bytes.len() {
            return None;
        }
        self.len = 0;
        let [flags, x, y] = self.bytes;
        if flags & (X_OVERFLOW | Y_OVERFLOW) != 0 {
            return None;
        }
        Some(Packet {
            dx: delta(x, flags & X_SIGN != 0),
            dy: delta(y, flags & Y_SIGN != 0),
            buttons: Buttons(flags & BUTTON_BITS),
        })
    }
}

/// A 9-bit two's complement delta: the low byte and its sign bit.
fn delta(low: u8, negative: bool) -> i16 {
    i16::from(low) - if negative { 256 } else { 0 }
}

/// A pointer position in pixels, kept on a `width` by `height` screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Cursor {
    /// At the top-left corner.
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    pub const fn x(&self) -> u32 {
        self.x
    }

    pub const fn y(&self) -> u32 {
        self.y
    }

    /// Moves by a packet's deltas, staying on the screen. Returns whether the
    /// pointer moved.
    pub fn apply(&mut self, packet: &Packet) -> bool {
        let x = step(self.x, i32::from(packet.dx), self.width);
        let y = step(self.y, -i32::from(packet.dy), self.height);
        let moved = (x, y) != (self.x, self.y);
        (self.x, self.y) = (x, y);
        moved
    }

    /// The text cell under the pointer as (row, column), for cells of `cell_width`
    /// by `cell_height` pixels. Pixels past the last whole cell belong to it.
    pub fn cell(&self, cell_width: u32, cell_height: u32) -> (u32, u32) {
        let (cell_width, cell_height) = (cell_width.max(1), cell_height.max(1));
        let rows = (self.height / cell_height).max(1);
        let cols = (self.width / cell_width).max(1);
        (
            (self.y / cell_height).min(rows - 1),
            (self.x / cell_width).min(cols - 1),
        )
    }
}

fn step(at: u32, delta: i32, size: u32) -> u32 {
    let last = i64::from(size.saturating_sub(1));
    (i64::from(at) + i64::from(delta)).clamp(0, last) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> ([Option<Packet>; 4], usize) {
        let mut decoder = Decoder::new();
        let mut packets = [None; 4];
        let mut count = 0;
        for &byte in bytes {
            if let Some(packet) = decoder.push(byte) {
                packets[count] = Some(packet);
                count += 1;
            }
        }
        (packets, count)
    }

    fn packet(dx: i16, dy: i16, buttons: u8) -> Option<Packet> {
        Some(Packet {
            dx,
            dy,
            buttons: Buttons(buttons),
        })
    }

    #[test]
    fn a_move_down_and_right_decodes() {
        // QEMU's `mouse_move 38 40`: right 38, down 40, so PS/2 Y is -40.
        let (packets, count) = decode(&[0x28, 38, 0xD8]);
        assert_eq!(count, 1);
        assert_eq!(packets[0], packet(38, -40, 0));
    }

    #[test]
    fn sign_bits_extend_to_nine_bits() {
        let (packets, count) = decode(&[0x18, 0xFF, 0x7F, 0x28, 0x00, 0x00, 0x08, 0xFF, 0xFF]);
        assert_eq!(count, 3);
        assert_eq!(packets[0], packet(-1, 127, 0));
        assert_eq!(packets[1], packet(0, -256, 0));
        assert_eq!(packets[2], packet(255, 255, 0));
    }

    #[test]
    fn a_first_byte_without_bit_3_is_dropped() {
        // A stream that starts mid-packet: 0x05 and 0x00 cannot start one.
        let (packets, count) = decode(&[0x05, 0x00, 0x09, 0x02, 0x03]);
        assert_eq!(count, 1);
        assert_eq!(packets[0], packet(2, 3, 1));
    }

    #[test]
    fn overflow_packets_are_dropped_whole() {
        let (packets, count) = decode(&[0x48, 0x10, 0x10, 0x88, 0x10, 0x10, 0x08, 0x01, 0x01]);
        assert_eq!(count, 1);
        assert_eq!(packets[0], packet(1, 1, 0));
    }

    #[test]
    fn buttons_are_read_from_the_low_bits() {
        let (packets, _) = decode(&[0x09, 0, 0, 0x0A, 0, 0, 0x0C, 0, 0]);
        let held = |i: usize, b| packets[i].unwrap().buttons.held(b);
        assert!(held(0, Button::Left) && !held(0, Button::Right));
        assert!(held(1, Button::Right) && !held(1, Button::Left));
        assert!(held(2, Button::Middle) && !held(2, Button::Left));
    }

    #[test]
    fn a_press_is_reported_once_until_released() {
        let none = Buttons::NONE;
        let left = Buttons(1);
        let left_right = Buttons(0b011);
        let presses = |now: Buttons, before| now.pressed_since(before).count();
        assert_eq!(left.pressed_since(none).next(), Some(Button::Left));
        assert_eq!(presses(left, left), 0);
        assert_eq!(left_right.pressed_since(left).next(), Some(Button::Right));
        assert_eq!(presses(left_right, left), 1);
        assert_eq!(presses(none, left_right), 0);
        assert_eq!(presses(Buttons(0b111), none), 3);
        assert_eq!(Button::Middle.number(), 2);
    }

    #[test]
    fn the_cursor_turns_ps2_y_into_screen_y() {
        let mut cursor = Cursor::new(1280, 800);
        assert!(cursor.apply(&Packet {
            dx: 38,
            dy: -40,
            buttons: Buttons::NONE
        }));
        assert_eq!((cursor.x(), cursor.y()), (38, 40));
        assert_eq!(cursor.cell(7, 16), (2, 5));
    }

    #[test]
    fn the_cursor_stays_on_the_screen() {
        let mut cursor = Cursor::new(100, 50);
        let moves = |dx, dy| Packet {
            dx,
            dy,
            buttons: Buttons::NONE,
        };
        assert!(
            !cursor.apply(&moves(-5, 5)),
            "already at the top-left corner"
        );
        assert!(cursor.apply(&moves(255, -255)));
        assert_eq!((cursor.x(), cursor.y()), (99, 49));
        assert!(!cursor.apply(&moves(1, -1)));
        // 100 / 7 is 14 whole columns and 50 / 16 is 3 whole rows.
        assert_eq!(cursor.cell(7, 16), (2, 13));
        assert!(cursor.apply(&moves(-99, 49)));
        assert_eq!(cursor.cell(7, 16), (0, 0));
    }

    #[test]
    fn a_zero_cell_size_does_not_divide_by_zero() {
        assert_eq!(Cursor::new(0, 0).cell(0, 0), (0, 0));
    }
}
