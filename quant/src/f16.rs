//! IEEE half precision in software: 1 sign bit, 5 exponent bits (bias 15) and 10
//! mantissa bits. Widening is exact; narrowing rounds to nearest, ties to even, as
//! the F16C instructions do, so a scale quantised here matches llama.cpp's.
//! [`widen`] widens a run of halves, with F16C in the SIMD tiers.

use crate::tier::{Kind, Tier};

/// Widens every half in `src` into `dst`, which has the same length. Widening is
/// exact, so every tier gives [`to_f32`]'s bits, except that F16C may quieten a
/// signalling NaN; [`from_f32`] never makes one.
pub fn widen(tier: Tier, src: &[u16], dst: &mut [f32]) {
    assert_eq!(src.len(), dst.len(), "widen lengths");
    match tier.0 {
        Kind::Baseline => {
            for (d, &s) in dst.iter_mut().zip(src) {
                *d = to_f32(s);
            }
        }
        // SAFETY: every SIMD tier needs AVX2 and F16C with the OS saving ymm
        // state (`tier::kinds`), and holding the tier proves the CPU has them.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 | Kind::Avx512 | Kind::Avx512Vnni => unsafe { widen_f16c(src, dst) },
    }
}

/// [`widen`] eight halves at a time with VCVTPH2PS, the tail one by one.
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
#[target_feature(enable = "avx2,f16c")]
unsafe fn widen_f16c(src: &[u16], dst: &mut [f32]) {
    use core::arch::x86_64::{__m128i, _mm_loadu_si128, _mm256_cvtph_ps, _mm256_storeu_ps};
    let (src8, src_tail) = src.as_chunks::<8>();
    let (dst8, dst_tail) = dst.as_chunks_mut::<8>();
    for (s, d) in src8.iter().zip(dst8) {
        // SAFETY: `s` is 16 readable bytes and `d` 32 writable ones; both loads
        // and stores are unaligned.
        unsafe {
            let halves = _mm_loadu_si128(s.as_ptr().cast::<__m128i>());
            _mm256_storeu_ps(d.as_mut_ptr(), _mm256_cvtph_ps(halves));
        }
    }
    for (d, &s) in dst_tail.iter_mut().zip(src_tail) {
        *d = to_f32(s);
    }
}

/// The f32 with the same value. NaN stays NaN.
pub fn to_f32(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exp = u32::from((h >> 10) & 0x1f);
    let mant = u32::from(h & 0x3ff);
    let bits = match (exp, mant) {
        (0, 0) => sign,
        // Subnormal: mant * 2^-24. Shift its top bit up to the implicit bit.
        (0, _) => {
            let shift = mant.leading_zeros() - 21;
            sign | ((113 - shift) << 23) | (((mant << shift) & 0x3ff) << 13)
        }
        (31, _) => sign | 0x7f80_0000 | (mant << 13),
        _ => sign | ((exp + 112) << 23) | (mant << 13),
    };
    f32::from_bits(bits)
}

/// The nearest half, ties to even. Too large becomes infinity; NaN becomes a quiet
/// NaN keeping the top of its payload.
pub fn from_f32(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 0xff {
        let nan = if mant != 0 {
            0x200 | (mant >> 13) as u16
        } else {
            0
        };
        return sign | 0x7c00 | nan;
    }
    // The exponent re-biased for a half.
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        // A half subnormal counts units of 2^-24; the value is m * 2^(e - 14) units.
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        return sign | round_shift(m, shift) as u16;
    }
    // A carry out of the mantissa bumps the exponent, up to infinity.
    let h = ((e as u32) << 10) | (mant >> 13);
    sign | round_tail(h, mant & 0x1fff, 0x1000) as u16
}

/// `value >> shift`, rounded to nearest with ties to even.
fn round_shift(value: u32, shift: u32) -> u32 {
    let tail = value & ((1 << shift) - 1);
    round_tail(value >> shift, tail, 1 << (shift - 1))
}

fn round_tail(kept: u32, tail: u32, half: u32) -> u32 {
    if tail > half || (tail == half && kept & 1 == 1) {
        kept + 1
    } else {
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert_eq!(to_f32(0x3c00), 1.0);
        assert_eq!(to_f32(0xc000), -2.0);
        assert_eq!(to_f32(0x7bff), 65504.0);
        assert_eq!(to_f32(0x0001), f32::from_bits(0x3380_0000)); // 2^-24
        assert_eq!(to_f32(0x0400), f32::from_bits(0x3880_0000)); // 2^-14
        assert_eq!(to_f32(0x7c00), f32::INFINITY);
        assert!(to_f32(0x7e00).is_nan());
        assert_eq!(from_f32(1.0), 0x3c00);
        assert_eq!(from_f32(65504.0), 0x7bff);
        assert_eq!(from_f32(65520.0), 0x7c00);
        assert_eq!(from_f32(-0.0), 0x8000);
        assert_eq!(from_f32(f32::from_bits(0x3300_0000)), 0); // 2^-25, a tie
        assert_eq!(from_f32(f32::from_bits(0x3340_0000)), 1); // 1.5 * 2^-25
        assert_eq!(from_f32(f32::from_bits(0x3f80_1000)), 0x3c00); // 1 + 2^-11, a tie
        assert_eq!(from_f32(f32::from_bits(0x3f80_3000)), 0x3c02); // 1 + 3 * 2^-11
        assert_eq!(from_f32(f32::NAN) & 0x7e00, 0x7e00);
    }

    #[test]
    fn every_tier_widens_every_half_exactly() {
        let src: alloc::vec::Vec<u16> = (0..=u16::MAX).collect();
        let mut want = alloc::vec![0.0f32; src.len()];
        widen(Tier::BASELINE, &src, &mut want);
        for tier in Tier::all() {
            // Odd lengths and offsets, so the tail and unaligned loads run too.
            for skip in [0, 3] {
                let src = &src[skip..src.len() - 5];
                let mut got = alloc::vec![0.0f32; src.len()];
                widen(tier, src, &mut got);
                for (i, (&h, (g, w))) in src.iter().zip(got.iter().zip(&want[skip..])).enumerate() {
                    let same = g.to_bits() == w.to_bits() || (g.is_nan() && w.is_nan());
                    assert!(same, "{} half {h:#06x} at {i}", tier.name());
                    assert_eq!(w.to_bits(), to_f32(h).to_bits());
                }
            }
        }
    }

    #[test]
    fn every_half_round_trips() {
        for h in 0..=u16::MAX {
            let f = to_f32(h);
            if f.is_nan() {
                assert!(h & 0x7c00 == 0x7c00 && h & 0x3ff != 0);
                continue;
            }
            assert_eq!(from_f32(f), h, "{h:#06x}");
        }
    }
}
