//! The kernel's random-number generator, behind ENTROPY: ChaCha20 keyed from
//! 32 bytes the CPU gives (RDSEED, or RDRAND without it), with fast key
//! erasure and periodic reseeding.
//!
//! - **Fast key erasure.** Every fill draws the caller's bytes and then 32 more
//!   that become the next key, so the state after a fill cannot reproduce
//!   anything already handed out.
//! - **Reseeding.** After [`RESEED_BYTES`] handed out or [`RESEED_MS`], the
//!   kernel mixes 32 fresh bytes from the CPU into the key. A failed hardware
//!   read then keeps the current key, which is still sound.
//!
//! The hardware reads stay in the kernel, so this is host-testable.

#![no_std]

use rand_chacha::ChaCha20Rng;
use rand_core::{Rng, SeedableRng};

/// Bytes in a seed, and in a fresh reseed.
pub const SEED_BYTES: usize = 32;
/// The most one ENTROPY call returns.
pub const MAX_REQUEST: usize = 256;
/// Reseed after this many bytes handed out.
pub const RESEED_BYTES: u64 = 1 << 20;
/// Reseed after this long, in milliseconds.
pub const RESEED_MS: u64 = 60_000;

pub struct Csprng {
    rng: ChaCha20Rng,
    /// Bytes handed out since the last seed.
    since_seed: u64,
    /// When it was last seeded, boot-relative.
    seeded_ms: u64,
}

impl Csprng {
    /// Keyed from `seed`, which must come from the hardware.
    pub fn new(seed: [u8; SEED_BYTES], now_ms: u64) -> Self {
        Self {
            rng: ChaCha20Rng::from_seed(seed),
            since_seed: 0,
            seeded_ms: now_ms,
        }
    }

    /// Fills `out` (at most [`MAX_REQUEST`] bytes, or it fills nothing and
    /// returns false), then moves to a key drawn from the same stream.
    pub fn fill(&mut self, out: &mut [u8]) -> bool {
        if out.len() > MAX_REQUEST {
            return false;
        }
        self.rng.fill_bytes(out);
        let mut key = [0u8; SEED_BYTES];
        self.rng.fill_bytes(&mut key);
        self.rng = ChaCha20Rng::from_seed(key);
        key.fill(0);
        self.since_seed += out.len() as u64;
        true
    }

    /// Whether it is time to mix in fresh bytes from the hardware.
    pub fn needs_reseed(&self, now_ms: u64) -> bool {
        self.since_seed >= RESEED_BYTES || now_ms.saturating_sub(self.seeded_ms) >= RESEED_MS
    }

    /// Mixes `fresh` hardware bytes into the key: the next key is the stream's
    /// next 32 bytes XOR `fresh`, so neither alone decides it.
    pub fn reseed(&mut self, fresh: [u8; SEED_BYTES], now_ms: u64) {
        let mut key = [0u8; SEED_BYTES];
        self.rng.fill_bytes(&mut key);
        for (k, f) in key.iter_mut().zip(fresh) {
            *k ^= f;
        }
        self.rng = ChaCha20Rng::from_seed(key);
        key.fill(0);
        self.since_seed = 0;
        self.seeded_ms = now_ms;
    }

    /// When the hardware read failed: try again only after another period.
    pub fn defer_reseed(&mut self, now_ms: u64) {
        self.since_seed = 0;
        self.seeded_ms = now_ms;
    }
}

#[cfg(test)]
mod tests;
