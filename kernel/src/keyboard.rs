//! PS/2 keyboard decoding, in normal kernel context, from the i8042 ring, through
//! `termcore::keys`. Mouse bytes met on the way go to the pointer with the Ctrl state
//! at that point in the ring, so a click knows whether Ctrl was held.

use crate::i8042::{self, Input};
use crate::{hpet, log, pointer, serial};
use alloc::string::String;
use termcore::keys::{self, Code, Key, Stroke};

pub struct Decoder {
    keys: keys::Decoder,
    /// The ring's epoch these keys were decoded in.
    epoch: u64,
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            keys: keys::Decoder::new(),
            epoch: i8042::epoch(),
        }
    }

    fn stroke(&mut self, input: Input) -> Option<Stroke> {
        // Keys changed hands since: what this decoder thinks is held is stale.
        let epoch = i8042::epoch();
        if epoch != self.epoch {
            self.keys = keys::Decoder::new();
            self.epoch = epoch;
        }
        if input.aux {
            pointer::on_byte(input.byte, self.keys.ctrl());
            return None;
        }
        self.keys.stroke(input.byte)
    }

    fn decode(&mut self, input: Input) -> Option<Key> {
        self.stroke(input).and_then(|stroke| stroke.key)
    }

    /// The keys held now.
    pub fn held(&self) -> impl Iterator<Item = Code> + '_ {
        self.keys.held()
    }

    /// Blocks the calling thread until a key goes down or up.
    pub fn read_stroke(&mut self) -> Stroke {
        loop {
            if let Some(stroke) = self.stroke(i8042::wait()) {
                return stroke;
            }
        }
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
