//! Quantisation, the f16 rounding against F16C, and bit-identical tiers.

extern crate std;

use crate::*;
use alloc::vec;
use alloc::vec::Vec;

/// splitmix64, so the tests need no crate and repeat exactly.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [-1, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

/// A Q8_0 row of random bytes with finite, random scales.
fn random_row(rng: &mut Rng, len: usize) -> Vec<u8> {
    let mut row = vec![0u8; row_bytes(len)];
    for block in row.as_chunks_mut::<BLOCK_BYTES>().0 {
        let scale = f16::from_f32(rng.unit() * 0.05);
        block[..2].copy_from_slice(&scale.to_le_bytes());
        for b in &mut block[2..] {
            *b = rng.next() as u8;
        }
    }
    row
}

fn random_vec(rng: &mut Rng, len: usize, magnitude: f32) -> QVec {
    let x: Vec<f32> = (0..len).map(|_| rng.unit() * magnitude).collect();
    let mut q = QVec::new(len);
    q.quantize(&x);
    q
}

/// Every tier's matvec, as bits.
fn all_tiers(w: &[u8], x: &QVec, rows: usize) -> Vec<Vec<u32>> {
    Tier::all()
        .map(|tier| {
            let mut y = vec![0.0f32; rows];
            matvec(tier, w, x, &mut y);
            y.iter().map(|v| v.to_bits()).collect()
        })
        .collect()
}

fn assert_tiers_agree(w: &[u8], x: &QVec, rows: usize) {
    // CI's runners have AVX2: without it there would be nothing to compare.
    if std::env::var_os("CI").is_some() {
        assert!(Tier::all().count() >= 2, "only the baseline tier runs here");
    }
    let results = all_tiers(w, x, rows);
    for (tier, bits) in Tier::all().zip(&results).skip(1) {
        assert_eq!(bits, &results[0], "{} differs from baseline", tier.name());
    }
}

#[test]
fn tiers_are_bit_identical_on_random_rows() {
    // 10,000 rows in all, at the real model's row lengths and two short ones.
    let mut rng = Rng(1);
    for (len, rows) in [(32, 4000), (64, 4000), (1024, 1500), (3072, 500)] {
        let w: Vec<u8> = (0..rows).flat_map(|_| random_row(&mut rng, len)).collect();
        let x = random_vec(&mut rng, len, 4.0);
        assert_tiers_agree(&w, &x, rows);
    }
}

#[test]
fn tiers_are_bit_identical_on_saturated_and_zero_blocks() {
    let len = 256;
    let x_max = {
        let mut q = QVec::new(len);
        let x: Vec<f32> = (0..len)
            .map(|i| if i % 3 == 0 { -1e3 } else { 1e3 })
            .collect();
        q.quantize(&x);
        q
    };
    assert!(x_max.qs.iter().all(|&q| q == 127 || q == -127));
    let mut rows = Vec::new();
    for fill in [-128i8, 127, 0, -1] {
        let mut row = vec![0u8; row_bytes(len)];
        for block in row.as_chunks_mut::<BLOCK_BYTES>().0 {
            block[..2].copy_from_slice(&f16::from_f32(1.5).to_le_bytes());
            block[2..].fill(fill as u8);
        }
        rows.push(row);
    }
    // Alternating -128 and 127, a zero scale, and an all-zero block.
    let mut row = vec![0u8; row_bytes(len)];
    for (i, block) in row.as_chunks_mut::<BLOCK_BYTES>().0.iter_mut().enumerate() {
        let scale = if i == 1 { 0.0 } else { 0.75 };
        block[..2].copy_from_slice(&f16::from_f32(scale).to_le_bytes());
        for (j, b) in block[2..].iter_mut().enumerate() {
            *b = if i == 2 {
                0
            } else if j % 2 == 0 {
                0x80
            } else {
                127
            };
        }
    }
    rows.push(row);
    let w: Vec<u8> = rows.concat();
    for x in [x_max, QVec::new(len), random_vec(&mut Rng(7), len, 100.0)] {
        assert_tiers_agree(&w, &x, rows.len());
    }
    // -128 against -127 everywhere: the largest products, summed exactly.
    let neg = {
        let mut q = QVec::new(len);
        q.quantize(&vec![-1.0; len]);
        q
    };
    assert!(neg.qs.iter().all(|&q| q == -127));
    let per_block = (1.5f32 * neg.scales[0]) * (32 * 128 * 127) as f32;
    let expected = (0..len / 32).fold(0.0f32, |t, _| t + per_block);
    let mut y = vec![0.0; 1];
    for tier in Tier::all() {
        matvec(tier, &rows[0], &neg, &mut y);
        assert_eq!(y[0], expected, "{}", tier.name());
    }
}

#[test]
fn tiers_agree_on_non_finite_scales() {
    // One row per odd scale in its first block, so no row's NaN hides another's
    // infinity: +inf, -inf, NaNs of both signs and payloads, a subnormal, -0, and
    // +inf times an all-zero block (inf * 0 is NaN). Then rows where two
    // different NaNs meet in the running total.
    let len = 128;
    let mut rng = Rng(3);
    let odd: [&[u16]; 9] = [
        &[0x7c00],
        &[0xfc00],
        &[0x7e00],
        &[0x7c01],
        &[0xfe00],
        &[0x0001],
        &[0x8000],
        &[0x7c01, 0xfe00],
        &[0xfe00, 0x7c01],
    ];
    let mut w = Vec::new();
    for scales in odd {
        let mut row = random_row(&mut rng, len);
        for (block, &scale) in row.as_chunks_mut::<BLOCK_BYTES>().0.iter_mut().zip(scales) {
            block[..2].copy_from_slice(&scale.to_le_bytes());
        }
        w.extend(row);
    }
    let mut zero_inf = random_row(&mut rng, len);
    zero_inf[..BLOCK_BYTES].fill(0);
    zero_inf[..2].copy_from_slice(&0x7c00u16.to_le_bytes());
    w.extend(zero_inf);
    let x = random_vec(&mut rng, len, 1.0);
    assert_tiers_agree(&w, &x, odd.len() + 1);
    let mut y = vec![0.0; odd.len() + 1];
    matvec(Tier::BASELINE, &w, &x, &mut y);
    assert!(y[0] == f32::INFINITY || y[0] == f32::NEG_INFINITY);
    assert_eq!(y[odd.len()].to_bits(), f32::NAN.to_bits());
    assert_eq!(y[7].to_bits(), f32::NAN.to_bits());
}

#[test]
fn matvec_matches_an_f64_reference() {
    let mut rng = Rng(5);
    let (len, rows) = (1024, 16);
    let w: Vec<u8> = (0..rows).flat_map(|_| random_row(&mut rng, len)).collect();
    let x = random_vec(&mut rng, len, 3.0);
    let mut xf = vec![0.0; len];
    x.dequantize(&mut xf);
    let mut wf = vec![0.0; len];
    let mut y = vec![0.0; rows];
    matvec(Tier::best(), &w, &x, &mut y);
    for (r, y) in y.iter().enumerate() {
        dequantize_row(&w[r * row_bytes(len)..(r + 1) * row_bytes(len)], &mut wf);
        let exact: f64 = wf
            .iter()
            .zip(&xf)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum();
        let scale: f64 = wf
            .iter()
            .zip(&xf)
            .map(|(a, b)| f64::from((a * b).abs()))
            .sum();
        assert!(
            (f64::from(*y) - exact).abs() <= 1e-5 * scale.max(1e-30),
            "row {r}"
        );
    }
}

#[test]
fn quantize_round_trips_within_half_a_step() {
    let mut rng = Rng(9);
    let x: Vec<f32> = (0..4096).map(|_| rng.unit() * 10.0).collect();
    let mut blocks = vec![0u8; row_bytes(x.len())];
    quantize_row(&x, &mut blocks);
    let mut back = vec![0.0; x.len()];
    dequantize_row(&blocks, &mut back);
    let mut q = QVec::new(x.len());
    q.quantize(&x);
    let mut back_q = vec![0.0; x.len()];
    q.dequantize(&mut back_q);
    assert_eq!(back, back_q);
    for (block, (x, y)) in x.chunks(32).zip(back.chunks(32)).enumerate() {
        // Half a step, plus up to 127 steps times the f16 scale's rounding (2^-11).
        let amax = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let bound = amax / 127.0 * (0.5 + 127.0 / 2048.0);
        for (a, b) in x.iter().zip(y) {
            assert!((a - b).abs() <= bound, "block {block}: {a} vs {b}");
        }
    }
}

#[test]
fn quantize_rounds_ties_to_even_and_never_writes_minus_128() {
    // amax 127 makes the multiplier exactly 1, so the values are their own codes.
    let mut x = vec![0.0f32; 32];
    x[0] = 127.0;
    x[1] = 2.5;
    x[2] = -2.5;
    x[3] = 3.5;
    x[4] = -127.0;
    x[5] = 0.5;
    let mut q = QVec::new(32);
    q.quantize(&x);
    assert_eq!(&q.qs[..6], [127, 2, -2, 4, -127, 0]);
    assert_eq!(q.scales[0], f16::to_f32(f16::from_f32(1.0)));
    // NaN quantises to 0 and an all-zero block to a zero scale.
    let mut x = vec![0.0f32; 64];
    x[0] = f32::NAN;
    q = QVec::new(64);
    q.quantize(&x);
    assert!(q.qs.iter().all(|&v| v == 0));
    assert_eq!(q.scales, [0.0, 0.0]);
    let mut rng = Rng(11);
    for _ in 0..100 {
        let x: Vec<f32> = (0..32).map(|_| rng.unit() * 1e6).collect();
        q = QVec::new(32);
        q.quantize(&x);
        assert!(q.qs.iter().all(|&v| v != -128));
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn f16_rounding_matches_f16c() {
    if !std::is_x86_feature_detected!("f16c") {
        return;
    }
    let mut rng = Rng(13);
    let check = |bits: u32| {
        let x = f32::from_bits(bits);
        // SAFETY: F16C was detected above.
        let hw = unsafe { hardware_f16(x) };
        assert_eq!(f16::from_f32(x), hw, "{bits:#010x}");
    };
    for bits in (0..=u32::MAX).step_by(65_521) {
        check(bits);
    }
    for _ in 0..200_000 {
        check(rng.next() as u32);
    }
    // Around every half's rounding boundary, the subnormal ones included: the
    // midpoint of two neighbouring halves is exact in f32.
    for h in 0..0x7bffu16 {
        let mid = (f64::from(f16::to_f32(h)) + f64::from(f16::to_f32(h + 1))) / 2.0;
        let mid = (mid as f32).to_bits();
        for bits in [mid - 1, mid, mid + 1] {
            check(bits);
            check(bits | 0x8000_0000);
        }
    }
    // Widening agrees with the hardware on every half but signalling NaNs, which
    // the hardware quiets.
    for h in 0..=u16::MAX {
        // SAFETY: F16C was detected above.
        let hw = unsafe { hardware_f32(h) };
        let ours = f16::to_f32(h);
        if ours.is_nan() {
            assert!(hw.is_nan(), "{h:#06x}");
        } else {
            assert_eq!(ours.to_bits(), hw.to_bits(), "{h:#06x}");
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "f16c")]
fn hardware_f32(h: u16) -> f32 {
    use core::arch::x86_64::*;
    _mm_cvtss_f32(_mm_cvtph_ps(_mm_set1_epi16(h as i16)))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "f16c")]
fn hardware_f16(x: f32) -> u16 {
    use core::arch::x86_64::*;
    let v = _mm_cvtps_ph::<_MM_FROUND_TO_NEAREST_INT>(_mm_set1_ps(x));
    _mm_extract_epi16::<0>(v) as u16
}

#[test]
fn tiers_are_named_and_the_baseline_comes_first() {
    let tiers: Vec<_> = Tier::all().collect();
    assert_eq!(tiers[0], Tier::BASELINE);
    assert_eq!(tiers[0].name(), "baseline");
    assert_eq!(Tier::best(), *tiers.last().unwrap());
    #[cfg(target_arch = "x86_64")]
    assert_eq!(
        Tier::avx2().is_some(),
        std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
    );
}
