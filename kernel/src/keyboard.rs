//! PS/2 keyboard decoding, in normal kernel context, from the i8042 ring. Mouse
//! bytes met on the way go to the pointer with the Ctrl state at that point in the
//! ring, so a click knows whether Ctrl was held.

use crate::i8042::{self, Input};
use crate::{hpet, log, pointer, serial};
use alloc::string::String;
use pc_keyboard::layouts::Us104Key;
use pc_keyboard::{DecodedKey, HandleControl, PS2Keyboard, ScancodeSet1};

pub struct Decoder(PS2Keyboard<Us104Key, ScancodeSet1>);

impl Decoder {
    pub fn new() -> Self {
        Self(PS2Keyboard::new(
            ScancodeSet1::new(),
            Us104Key,
            HandleControl::Ignore,
        ))
    }

    fn decode(&mut self, input: Input) -> Option<char> {
        if input.aux {
            pointer::on_byte(input.byte, self.0.get_modifiers().is_ctrl());
            return None;
        }
        match self.0.add_byte(input.byte) {
            Ok(Some(event)) => match self.0.process_keyevent(event) {
                Some(DecodedKey::Unicode(ch)) => Some(ch),
                _ => None,
            },
            _ => None,
        }
    }

    /// Decodes queued bytes until one produces a character.
    pub fn next_char(&mut self) -> Option<char> {
        while let Some(input) = i8042::next() {
            if let Some(ch) = self.decode(input) {
                return Some(ch);
            }
        }
        None
    }

    /// Drops every queued key, still decoding each so Shift and the like stay in
    /// step. Used before a question, so keys typed earlier cannot answer it.
    pub fn discard_pending(&mut self) {
        while let Some(input) = i8042::next() {
            let _ = self.decode(input);
        }
    }

    /// Blocks the calling thread until a key produces a character.
    pub fn read_char(&mut self) -> char {
        loop {
            if let Some(ch) = self.decode(i8042::wait()) {
                return ch;
            }
        }
    }
}

/// M3 exit test: the harness types `kai` and Enter; the kernel must echo and receive it.
pub fn typing_check() -> Result<(), &'static str> {
    const TIMEOUT_NS: u64 = 30_000_000_000;
    log!("keyboard: ready for keys, waiting for \"kai\" and Enter");
    let mut decoder = Decoder::new();
    let mut line = String::new();
    let start = hpet::nanos();
    loop {
        while let Some(ch) = decoder.next_char() {
            serial::_print(format_args!("{ch}"));
            if matches!(ch, '\n' | '\r') {
                if line == "kai" {
                    log!("keyboard: received \"kai\"");
                    return Ok(());
                }
                line.clear();
            } else {
                line.push(ch);
            }
        }
        if hpet::nanos() - start > TIMEOUT_NS {
            return Err("keyboard: no \"kai\" line typed within 30 s");
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}
