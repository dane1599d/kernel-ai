//! Reading memory as fast as the CPU can: the roofline `cargo xtask bench quant`
//! measures the matrix-vector products against. It lives here rather than in
//! xtask so it builds optimised in dev builds, like the products themselves.

use crate::tier::{Kind, Tier};

/// Reads every byte of `bytes` once, front to back, and returns a sum of them as
/// u64 words (wrapping), so no read can be left out. Four independent sums keep
/// the loads from waiting on one another; nothing is prefetched by hand.
pub fn read(tier: Tier, bytes: &[u8]) -> u64 {
    let (words, tail) = bytes.as_chunks::<8>();
    let tail = tail.iter().fold(0u64, |s, &b| s.wrapping_add(u64::from(b)));
    let body = match tier.0 {
        Kind::Baseline => read_words(words),
        // SAFETY: every SIMD tier exists only when the CPU has AVX2 and the OS
        // saves ymm state; see `tier::kinds`.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 | Kind::Avx512 | Kind::Avx512Vnni => unsafe { avx2::read(bytes) },
    };
    body.wrapping_add(tail)
}

fn read_words(words: &[[u8; 8]]) -> u64 {
    let mut sums = [0u64; 4];
    let (quads, rest) = words.as_chunks::<4>();
    for quad in quads {
        for (s, w) in sums.iter_mut().zip(quad) {
            *s = s.wrapping_add(u64::from_le_bytes(*w));
        }
    }
    let rest = rest
        .iter()
        .fold(0u64, |s, w| s.wrapping_add(u64::from_le_bytes(*w)));
    sums.iter().fold(rest, |a, &s| a.wrapping_add(s))
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
mod avx2 {
    use core::arch::x86_64::*;

    /// The whole 32-byte blocks of `bytes`, 128 bytes a step into four sums; the
    /// last few words the scalar way.
    #[target_feature(enable = "avx2")]
    pub fn read(bytes: &[u8]) -> u64 {
        let (steps, rest) = bytes.as_chunks::<128>();
        let mut sums = [_mm256_setzero_si256(); 4];
        for step in steps {
            for (s, part) in sums.iter_mut().zip(step.as_chunks::<32>().0) {
                // SAFETY: 32 readable bytes; loadu has no alignment rule. The
                // tier was dispatched with AVX2.
                let v = unsafe { _mm256_loadu_si256(part.as_ptr().cast()) };
                *s = _mm256_add_epi64(*s, v);
            }
        }
        let mut lanes = [0u64; 4];
        let total = _mm256_add_epi64(
            _mm256_add_epi64(sums[0], sums[1]),
            _mm256_add_epi64(sums[2], sums[3]),
        );
        // SAFETY: 32 writable bytes; storeu has no alignment rule. AVX2 as above.
        unsafe { _mm256_storeu_si256(lanes.as_mut_ptr().cast(), total) };
        let words = rest.as_chunks::<8>().0;
        lanes
            .iter()
            .fold(super::read_words(words), |a, &s| a.wrapping_add(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ksupport::Rng;
    use alloc::vec::Vec;

    #[test]
    fn every_tier_reads_every_byte() {
        let mut rng = Rng(51);
        let bytes: Vec<u8> = (0..4099).map(|_| rng.next() as u8).collect();
        for len in (0..300).chain([1023, 1024, 4096, 4099]) {
            let bytes = &bytes[..len];
            let words = bytes.as_chunks::<8>();
            let want = words
                .0
                .iter()
                .map(|w| u64::from_le_bytes(*w))
                .chain(words.1.iter().map(|&b| u64::from(b)))
                .fold(0u64, u64::wrapping_add);
            for tier in Tier::all() {
                assert_eq!(read(tier, bytes), want, "{} on {len} bytes", tier.name());
            }
        }
    }
}
