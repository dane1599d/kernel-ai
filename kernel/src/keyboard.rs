//! PS/2 keyboard decoding, in normal kernel context, from the i8042 ring, through
//! `termcore::keys`. Mouse bytes met on the way go to the pointer with the Ctrl state
//! at that point in the ring, so a click knows whether Ctrl was held.

use crate::i8042::{self, Input};
use crate::{hpet, log, pointer, serial};
use alloc::string::String;
use termcore::keys::{self, Key};

pub struct Decoder(keys::Decoder);

impl Decoder {
    pub fn new() -> Self {
        Self(keys::Decoder::new())
    }

    fn decode(&mut self, input: Input) -> Option<Key> {
        if input.aux {
            pointer::on_byte(input.byte, self.0.ctrl());
            return None;
        }
        self.0.feed(input.byte)
    }

    /// Decodes queued bytes until one produces a key.
    pub fn next_key(&mut self) -> Option<Key> {
        while let Some(input) = i8042::next() {
            if let Some(key) = self.decode(input) {
                return Some(key);
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

    /// Blocks the calling thread until a key comes.
    pub fn read_key(&mut self) -> Key {
        loop {
            if let Some(key) = self.decode(i8042::wait()) {
                return key;
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
        while let Some(key) = decoder.next_key() {
            match key {
                Key::Enter => {
                    serial::_print(format_args!("\n"));
                    if line == "kai" {
                        log!("keyboard: received \"kai\"");
                        return Ok(());
                    }
                    line.clear();
                }
                Key::Char(ch) => {
                    serial::_print(format_args!("{ch}"));
                    line.push(ch);
                }
                _ => {}
            }
        }
        if hpet::nanos() - start > TIMEOUT_NS {
            return Err("keyboard: no \"kai\" line typed within 30 s");
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}
