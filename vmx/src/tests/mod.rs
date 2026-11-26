//! Host tests: each encoding against the SDM's tables (version 093), and
//! every decoder against hostile input, which it refuses without a panic.

mod bitmap;
mod controls;
mod ept;
mod event;
mod exit;
mod field;
mod hostile;
mod vmcs;

/// A xorshift64 stream: the hostile tests' inputs, the same every run.
pub(crate) struct Noise(pub u64);

impl Noise {
    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}
