//! `test=entropy-boots`, P5 task 3: ENTROPY answers, its first 16 bytes are
//! logged, and xtask boots it twice and checks the two differ
//! (`TestCase::differs`). The case attaches a virtio-rng, which must have been
//! mixed in.

use crate::log;
use alloc::string::String;
use core::fmt::Write;

const SAMPLE_BYTES: usize = 16;

pub fn check() -> Result<(), &'static str> {
    let mut sample = [0u8; SAMPLE_BYTES];
    if !crate::entropy::fill(&mut sample) {
        return Err("entropy-boots: ENTROPY refused");
    }
    let mut hex = String::new();
    for byte in sample {
        let _ = write!(hex, "{byte:02x}");
    }
    log!("entropy-boots: sample {hex}");
    Ok(())
}
