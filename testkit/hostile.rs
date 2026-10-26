//! Shared helpers for the hostile-input tests of the parsers of untrusted bytes
//! (gguf, bpe, uelf, llmproto, acpitables, wasihost). Not a crate: each crate's
//! integration test includes this file with `#[path = "../../testkit/hostile.rs"] mod hostile;`,
//! so no dependency is added anywhere.
//!
//! It gives a deterministic xorshift PRNG, byte mutations of a valid sample, a
//! sweep that writes interesting values into every field position, and a
//! counting global allocator, so a test can assert both "returns Ok or Err, never
//! panics" and "allocates no more than so much".

#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// Marsaglia's xorshift64: fixed seeds give the same cases on every run.
pub struct XorShift(u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        // Zero is xorshift's one fixed point.
        Self(seed | 1)
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number below `n`, which must not be zero.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Values that break length and count arithmetic: zero, one, powers of two near
/// the type limits, and the limits themselves.
pub const INTERESTING: [u64; 14] = [
    0,
    1,
    2,
    7,
    0x7f,
    0xff,
    0x1000,
    0xffff,
    0x7fff_ffff,
    0xffff_ffff,
    1 << 32,
    1 << 62,
    i64::MAX as u64,
    u64::MAX,
];

/// `sample` with one to four random edits: a random byte, a flipped bit, an
/// extreme byte, an interesting little-endian value of 2, 4 or 8 bytes at a random
/// place, a byte inserted or removed, or the tail cut off.
pub fn mutate(rng: &mut XorShift, sample: &[u8]) -> Vec<u8> {
    let mut bytes = sample.to_vec();
    for _ in 0..1 + rng.below(4) {
        if bytes.is_empty() {
            bytes.push(rng.next() as u8);
            continue;
        }
        let at = rng.below(bytes.len());
        match rng.below(8) {
            0 => bytes[at] = rng.next() as u8,
            1 => bytes[at] ^= 1 << rng.below(8),
            2 => bytes[at] = [0, 0xff, 0x7f, 0x80][rng.below(4)],
            3..=5 => {
                let value = INTERESTING[rng.below(INTERESTING.len())]
                    .wrapping_add(rng.below(3) as u64)
                    .wrapping_sub(1);
                let width = [2, 4, 8][rng.below(3)];
                put(&mut bytes, at, value, width);
            }
            6 => {
                if rng.below(2) == 0 {
                    bytes.insert(at, rng.next() as u8);
                } else {
                    bytes.remove(at);
                }
            }
            _ => bytes.truncate(at),
        }
    }
    bytes
}

/// Calls `f` on `cases` mutations of `sample` from `seed`.
pub fn fuzz(sample: &[u8], seed: u64, cases: usize, mut f: impl FnMut(&[u8])) {
    let mut rng = XorShift::new(seed);
    for _ in 0..cases {
        f(&mutate(&mut rng, sample));
    }
}

/// Calls `f` on every prefix of `sample`, the empty one and the whole included.
pub fn truncations(sample: &[u8], mut f: impl FnMut(usize, &[u8])) {
    for cut in 0..=sample.len() {
        f(cut, &sample[..cut]);
    }
}

/// Calls `f` on `sample` with each [`INTERESTING`] value written as a 4- and an
/// 8-byte little-endian field at every offset in `offsets`: every length, count
/// and offset field there is hit with every extreme value.
pub fn sweep_fields(
    sample: &[u8],
    offsets: core::ops::Range<usize>,
    mut f: impl FnMut(usize, &[u8]),
) {
    let mut bytes = sample.to_vec();
    for at in offsets {
        for value in INTERESTING {
            for width in [4, 8] {
                bytes.copy_from_slice(sample);
                put(&mut bytes, at, value, width);
                f(at, &bytes);
            }
        }
    }
}

fn put(bytes: &mut [u8], at: usize, value: u64, width: usize) {
    for (i, b) in value.to_le_bytes()[..width].iter().enumerate() {
        if let Some(slot) = bytes.get_mut(at + i) {
            *slot = *b;
        }
    }
}

/// Counts the bytes each thread has allocated, so tests running in parallel do
/// not see each other's allocations.
struct Counting;

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

fn grew(size: usize) {
    let _ = LIVE.try_with(|live| {
        let now = live.get().saturating_add(size);
        live.set(now);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
    });
}

fn shrank(size: usize) {
    let _ = LIVE.try_with(|live| live.set(live.get().saturating_sub(size)));
}

// SAFETY: every call goes straight to the system allocator; the counters are
// thread-local cells that never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout is passed on unchanged.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grew(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` above with this layout.
        unsafe { System.dealloc(ptr, layout) };
        shrank(layout.size());
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// What `f` returns, and the most bytes it had allocated at once on this thread.
pub fn peak_alloc<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let base = LIVE.with(Cell::get);
    PEAK.with(|peak| peak.set(base));
    let out = f();
    (out, PEAK.with(Cell::get).saturating_sub(base))
}
