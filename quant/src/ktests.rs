//! Q4_K and Q6_K: the layout against real rows, bit-identical tiers on random,
//! saturated, all-zero and non-finite blocks, and the products against an f64
//! reference.

extern crate std;

use crate::ksupport::*;
use crate::*;
use alloc::vec;
use alloc::vec::Vec;

fn read_bf16(bytes: &[u8]) -> Vec<f32> {
    let pairs = bytes.as_chunks::<2>().0;
    pairs
        .iter()
        .map(|b| f32::from_bits(u32::from(u16::from_le_bytes(*b)) << 16))
        .collect()
}

/// Row 512 of `blk.0.attn_k.weight` (Q4_K) and of `blk.0.attn_v.weight` (Q6_K)
/// from unsloth/Qwen3-0.6B-GGUF at 50968a4468ef4233ed78cd7c3de230dd1d61a56b
/// (Apache-2.0), with the same rows of its BF16 file. Dequantised by the layout
/// above, every value lies within 3 quantisation steps of the original weight,
/// each sub-block's mean error within half a step (0.09 at worst here), and the
/// rows point the same way. A code or scale taken from the wrong place is many
/// steps off, and a Q4_K minimum from the wrong sub-block shifts its whole
/// sub-block: each of the 28 swaps of two minimums is 1.4 steps off or more.
#[test]
fn dequantisation_matches_real_rows() {
    let cases: [(Format, &[u8], &[u8]); 2] = [
        (
            Format::Q4K,
            include_bytes!("../testdata/q4_k-row.bin"),
            include_bytes!("../testdata/q4_k-row.bf16"),
        ),
        (
            Format::Q6K,
            include_bytes!("../testdata/q6_k-row.bin"),
            include_bytes!("../testdata/q6_k-row.bf16"),
        ),
    ];
    for (format, row, bf16) in cases {
        let truth = read_bf16(bf16);
        let mut ours = vec![0.0f32; truth.len()];
        format.dequantize(row, &mut ours);
        let sub_len = match format {
            Format::Q4K => 32,
            Format::Q6K => 16,
        };
        let blocks = row.chunks_exact(format.block_bytes());
        for (b, block) in blocks.enumerate() {
            let step = |i: usize| match format {
                Format::Q4K => {
                    let (d, _, sc, _) = q4_k::header(block.try_into().unwrap());
                    d * f32::from(sc[i / 32])
                }
                Format::Q6K => {
                    let (d, sc) = q6_k::header(block.try_into().unwrap());
                    (d * f32::from(sc[i / 16])).abs()
                }
            };
            for i in 0..SUPER_LEN {
                let at = b * SUPER_LEN + i;
                let off = (ours[at] - truth[at]).abs();
                assert!(off <= 3.0 * step(i), "{format:?} value {at}: {off}");
            }
            for start in (0..SUPER_LEN).step_by(sub_len) {
                let at = b * SUPER_LEN + start;
                let mean = (at..at + sub_len)
                    .map(|i| f64::from(ours[i] - truth[i]))
                    .sum::<f64>()
                    / sub_len as f64;
                let bound = 0.5 * f64::from(step(start));
                assert!(mean.abs() <= bound, "{format:?} sub-block at {at}: {mean}");
            }
        }
        let dot = |a: &[f32], b: &[f32]| -> f64 {
            a.iter()
                .zip(b)
                .map(|(x, y)| f64::from(*x) * f64::from(*y))
                .sum()
        };
        let cos = dot(&ours, &truth) / (dot(&ours, &ours) * dot(&truth, &truth)).sqrt();
        assert!(cos > 0.99, "{format:?}: cosine {cos}");
    }
}

#[test]
fn tiers_are_bit_identical_on_random_rows() {
    // The 0.6B's, 1.7B's and 4B's row lengths, and 1, 3 and 5 super-blocks.
    let mut rng = Rng(21);
    let shapes = [
        (256, 400),
        (768, 150),
        (1280, 90),
        (1024, 200),
        (2048, 60),
        (2560, 60),
    ];
    for format in Format::ALL {
        for (len, rows) in shapes.into_iter().chain([(6144, 20), (9728, 12)]) {
            let w = random_rows(&mut rng, format, len, rows);
            let x = random_x(&mut rng, len, 4.0);
            assert_tiers_agree(format, &w, &x, rows);
        }
    }
}

#[test]
fn tiers_are_bit_identical_on_saturated_and_zero_blocks() {
    let len = 512;
    // x at +-127 in every byte, then all zero.
    let mut plus_minus = Q8KVec::new(len);
    let pm: Vec<f32> = (0..len)
        .map(|i| if i % 3 == 0 { -5.0 } else { 5.0 })
        .collect();
    plus_minus.quantize(&pm);
    assert!(plus_minus.qs.iter().all(|&q| q == 127 || q == -127));
    let mut minus = Q8KVec::new(len);
    minus.quantize(&vec![-1.0; len]);
    let xs = [plus_minus, minus, Q8KVec::new(len)];
    for format in Format::ALL {
        let mut rows = Vec::new();
        for fill in [0xffu8, 0x00, 0x80, 0x7f, 0x55] {
            let mut row = vec![fill; format.row_bytes(len)];
            for block in row.chunks_exact_mut(format.block_bytes()) {
                for &at in format.scale_offsets() {
                    block[at..at + 2].copy_from_slice(&f16::from_f32(65504.0).to_le_bytes());
                }
            }
            rows.push(row);
        }
        // An all-zero row: zero scales and codes.
        rows.push(vec![0; format.row_bytes(len)]);
        let w = rows.concat();
        for x in &xs {
            assert_tiers_agree(format, &w, x, rows.len());
        }
    }
}

#[test]
fn tiers_agree_on_non_finite_scales() {
    // One row per odd pair of scales, then random raw scales everywhere.
    let len = 512;
    let odd: [u16; 7] = [0x7c00, 0xfc00, 0x7e00, 0x7c01, 0xfe00, 0x0001, 0x8000];
    let mut rng = Rng(23);
    for format in Format::ALL {
        let mut w = Vec::new();
        for (i, &scale) in odd.iter().enumerate() {
            let mut row = random_rows(&mut rng, format, len, 1);
            let block = &mut row[..format.block_bytes()];
            let offsets = format.scale_offsets();
            block[offsets[0]..offsets[0] + 2].copy_from_slice(&scale.to_le_bytes());
            if let Some(&at) = offsets.get(1) {
                let other = odd[(i + 3) % odd.len()];
                block[at..at + 2].copy_from_slice(&other.to_le_bytes());
            }
            w.extend(row);
        }
        let raw: Vec<u8> = (0..format.row_bytes(len) * 8)
            .map(|_| rng.next() as u8)
            .collect();
        w.extend(raw);
        let rows = odd.len() + 8;
        let x = random_x(&mut rng, len, 1.0);
        assert_tiers_agree(format, &w, &x, rows);
        let mut y = vec![0.0; rows];
        format.matvec(Tier::BASELINE, &w, &x, &mut y);
        assert!(y.iter().any(|v| v.is_nan()));
        assert!(
            y.iter()
                .all(|v| !v.is_nan() || v.to_bits() == f32::NAN.to_bits())
        );
    }
}

/// The size each product's rounding is measured against: |w * x| summed, where
/// for Q4_K |w| counts its two terms apart (d * sc * q and dmin * m), which can
/// cancel in w but not in the rounding.
fn magnitude(format: Format, row: &[u8], wf: &[f32], xf: &[f32]) -> f64 {
    let blocks = row.chunks_exact(format.block_bytes());
    let mut total = 0.0f64;
    for (b, block) in blocks.enumerate() {
        for i in 0..SUPER_LEN {
            let at = b * SUPER_LEN + i;
            let min = match format {
                Format::Q4K => {
                    let (_, dmin, _, m) = q4_k::header(block.try_into().unwrap());
                    2.0 * f64::from(dmin * f32::from(m[i / 32])).abs()
                }
                Format::Q6K => 0.0,
            };
            total += (f64::from(wf[at]).abs() + min) * f64::from(xf[at]).abs();
        }
    }
    total
}

#[test]
fn products_match_an_f64_reference() {
    let mut rng = Rng(25);
    let len = 2560;
    for format in Format::ALL {
        let mut w = random_rows(&mut rng, format, len, 12);
        // Saturated rows too: every byte 0xff, then alternating 0x80 and 0x7f,
        // with finite scales.
        for fill in [[0xffu8, 0xff], [0x80, 0x7f]] {
            let mut row: Vec<u8> = (0..format.row_bytes(len)).map(|i| fill[i % 2]).collect();
            for block in row.chunks_exact_mut(format.block_bytes()) {
                for &at in format.scale_offsets() {
                    block[at..at + 2].copy_from_slice(&f16::from_f32(-0.003).to_le_bytes());
                }
            }
            w.extend(row);
        }
        let rows = 14;
        let row = format.row_bytes(len);
        let mut wf = vec![0.0; len];
        for x in [random_x(&mut rng, len, 3.0), random_x(&mut rng, len, 1e3)] {
            let mut xf = vec![0.0; len];
            x.dequantize(&mut xf);
            for tier in Tier::all() {
                let mut y = vec![0.0; rows];
                format.matvec(tier, &w, &x, &mut y);
                for (r, y) in y.iter().enumerate() {
                    let w = &w[r * row..(r + 1) * row];
                    format.dequantize(w, &mut wf);
                    let exact: f64 = wf
                        .iter()
                        .zip(&xf)
                        .map(|(a, b)| f64::from(*a) * f64::from(*b))
                        .sum();
                    let bound = 1e-5 * magnitude(format, w, &wf, &xf).max(1e-30);
                    let off = (f64::from(*y) - exact).abs();
                    let name = tier.name();
                    assert!(off <= bound, "{format:?} {name} row {r}: {off} > {bound}");
                }
            }
        }
    }
}

/// The largest integers a super-block can reach, in every tier: Q4_K's a and b
/// with every code 15, every scale and minimum 63 and x at +-127, and Q6_K's s
/// with every scale -128 against x at 127 (codes 0) and -127 (codes 63). Then
/// odd ones above 2^24, which an f32 cannot hold: a tier that passed one through
/// f32 would lose its last bit.
#[test]
fn every_tier_reaches_the_extreme_integers_exactly() {
    let x_at = |v: f32| {
        let mut x = Q8KVec::new(512);
        x.quantize(&vec![v; 512]);
        x
    };
    let (plus, minus) = (x_at(1.0), x_at(-1.0));
    assert!(plus.qs.iter().all(|&q| q == 127) && minus.qs.iter().all(|&q| q == -127));
    let mut q4 = vec![0xffu8; q4_k::row_bytes(512)];
    for block in q4.as_chunks_mut::<{ q4_k::BLOCK_BYTES }>().0 {
        block[..4].copy_from_slice(&[0, 0x3c, 0, 0x3c]);
    }
    let mut q6_zero = vec![0u8; q6_k::row_bytes(512)];
    let mut q6_full = vec![0xffu8; q6_k::row_bytes(512)];
    for row in [&mut q6_zero, &mut q6_full] {
        for block in row.as_chunks_mut::<{ q6_k::BLOCK_BYTES }>().0 {
            block[q6_k::SCALES..q6_k::D].fill(0x80);
            block[q6_k::D..].copy_from_slice(&[0, 0x3c]);
        }
    }
    let (a, b) = (8 * 63 * 32 * 15 * 127, 8 * 63 * 32 * 127);
    assert_eq!((a, b), (30_723_840, 2_048_256));
    type Case<'a> = (Format, &'a [u8], &'a Q8KVec, (i32, i32));
    let cases: [Case; 4] = [
        (Format::Q4K, &q4, &plus, (a, b)),
        (Format::Q4K, &q4, &minus, (-a, -b)),
        (Format::Q6K, &q6_zero, &plus, (133_169_152, 0)),
        (
            Format::Q6K,
            &q6_full,
            &minus,
            (16 * -128 * (31 * -127 * 16), 0),
        ),
    ];
    // Q4_K: one code of 14 instead of 15 takes 63 * 127 off. Q6_K: scales of
    // -127 and one code of 1 instead of 0 take 127 * 127 off.
    let mut q4_odd = q4.clone();
    for block in q4_odd.as_chunks_mut::<{ q4_k::BLOCK_BYTES }>().0 {
        block[q4_k::CODES] = 0xfe;
    }
    let mut q6_odd = q6_zero.clone();
    for block in q6_odd.as_chunks_mut::<{ q6_k::BLOCK_BYTES }>().0 {
        block[q6_k::SCALES..q6_k::D].fill(0x81);
        block[0] = 1;
    }
    let odd: [Case; 2] = [
        (Format::Q4K, &q4_odd, &plus, (30_715_839, b)),
        (Format::Q6K, &q6_odd, &plus, (132_112_639, 0)),
    ];
    for (format, w, x, want) in cases.into_iter().chain(odd) {
        for tier in Tier::all() {
            assert_eq!(
                format.ints(tier, w, x),
                [want; 2],
                "{format:?} {}",
                tier.name()
            );
        }
        assert_tiers_agree(format, w, x, 1);
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn tiers_follow_the_cpu_and_the_os() {
    use cpufeat::{Features, XCR0_AVX, XCR0_HI16_ZMM, XCR0_OPMASK, XCR0_SSE, XCR0_X87};
    let ymm = XCR0_X87 | XCR0_SSE | XCR0_AVX;
    let zmm = ymm | XCR0_OPMASK | cpufeat::XCR0_ZMM_HI256 | XCR0_HI16_ZMM;
    let all = Features {
        avx: true,
        avx2: true,
        fma: true,
        f16c: true,
        avx512f: true,
        avx512bw: true,
        avx512vl: true,
        avx512vnni: true,
        xcr0: zmm,
        ..Features::default()
    };
    let kinds = |cpu: Features| crate::tier::kinds(&cpu);
    assert_eq!(kinds(all), [true, true, true]);
    let no_vnni = Features {
        avx512vnni: false,
        ..all
    };
    assert_eq!(kinds(no_vnni), [true, true, false]);
    // The OS saves only ymm, or only xmm.
    assert_eq!(kinds(Features { xcr0: ymm, ..all }), [true, false, false]);
    let xmm = XCR0_X87 | XCR0_SSE;
    assert_eq!(kinds(Features { xcr0: xmm, ..all }), [false, false, false]);
    // A hypervisor may hide any bit: VL, BW, F16C, FMA, or AVX2 under AVX-512.
    let without = |cpu: Features| kinds(cpu);
    assert_eq!(
        without(Features {
            avx512vl: false,
            ..all
        }),
        [true, false, false]
    );
    assert_eq!(
        without(Features {
            avx512bw: false,
            ..all
        }),
        [true, false, false]
    );
    assert_eq!(
        without(Features { f16c: false, ..all }),
        [false, false, false]
    );
    assert_eq!(
        without(Features { fma: false, ..all }),
        [false, false, false]
    );
    assert_eq!(
        without(Features { avx2: false, ..all }),
        [false, false, false]
    );
}

#[test]
fn quantize_round_trips_within_a_step() {
    let mut rng = Rng(27);
    let x: Vec<f32> = (0..4096).map(|_| rng.unit() * 3.0).collect();
    for format in Format::ALL {
        let mut blocks = vec![0u8; format.row_bytes(x.len())];
        format.quantize(&x, &mut blocks);
        let mut back = vec![0.0; x.len()];
        format.dequantize(&blocks, &mut back);
        // Q4_K has 15 steps a sub-block, Q6_K 63; the 6- and 8-bit scales and
        // the f16 super-scales round a little more.
        let (sub_len, levels) = match format {
            Format::Q4K => (32, 15.0),
            Format::Q6K => (16, 62.0),
        };
        for (sub, (x, y)) in x.chunks(sub_len).zip(back.chunks(sub_len)).enumerate() {
            let span = x.iter().fold(0.0f32, |m, v| m.max(v.abs())) * 2.0;
            for (a, b) in x.iter().zip(y) {
                assert!(
                    (a - b).abs() <= span / levels,
                    "{format:?} {sub}: {a} vs {b}"
                );
            }
        }
    }
    // Zeros stay zero.
    let mut q4 = vec![0xaa; q4_k::row_bytes(256)];
    q4_k::quantize_row(&[0.0; 256], &mut q4);
    let mut q6 = vec![0xaa; q6_k::row_bytes(256)];
    q6_k::quantize_row(&[0.0; 256], &mut q6);
    let mut back = [1.0f32; 256];
    q4_k::dequantize_row(&q4, &mut back);
    assert!(back.iter().all(|&v| v == 0.0));
    q6_k::dequantize_row(&q6, &mut back);
    assert!(back.iter().all(|&v| v == 0.0));
}

#[test]
fn q8k_quantises_with_sums_and_never_writes_minus_128() {
    let mut rng = Rng(29);
    let x: Vec<f32> = (0..1024).map(|_| rng.unit() * 1e4).collect();
    let mut q = Q8KVec::new(1024);
    q.quantize(&x);
    assert!(q.qs.iter().all(|&v| v != -128));
    for (j, sum) in q.sums.iter().enumerate() {
        let want: i32 = q.qs[16 * j..16 * j + 16]
            .iter()
            .map(|&v| i32::from(v))
            .sum();
        assert_eq!(i32::from(*sum), want);
    }
    let mut back = vec![0.0; 1024];
    q.dequantize(&mut back);
    for (block, (x, y)) in x.chunks(256).zip(back.chunks(256)).enumerate() {
        let amax = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        for (a, b) in x.iter().zip(y) {
            assert!((a - b).abs() <= amax / 127.0 * 0.5001, "block {block}");
        }
    }
    // NaN quantises to 0; an infinity gives an infinite scale and zero bytes.
    let mut x = vec![0.5f32; 256];
    x[3] = f32::NAN;
    q = Q8KVec::new(256);
    q.quantize(&x);
    assert_eq!(q.qs[3], 0);
    x[4] = f32::INFINITY;
    q.quantize(&x);
    assert!(q.scales[0].is_infinite() && q.qs.iter().all(|&v| v == 0));
}

#[cfg(target_arch = "x86_64")]
#[test]
fn every_tier_the_cpu_offers_is_tested() {
    use std::is_x86_feature_detected as has;
    let names: Vec<_> = Tier::all().map(Tier::name).collect();
    let avx2 = has!("avx2") && has!("fma") && has!("f16c");
    let avx512 = avx2 && has!("avx512f") && has!("avx512bw") && has!("avx512vl");
    let vnni = avx512 && has!("avx512vnni");
    let mut want = vec!["baseline"];
    want.extend(avx2.then_some("avx2+fma"));
    want.extend(avx512.then_some("avx512"));
    want.extend(vnni.then_some("avx512+vnni"));
    assert_eq!(names, want);
    std::eprintln!("quant tiers on this CPU: {}", names.join(", "));
    // A machine known to have every tier (the 7700X: KAI_EXPECT_TIERS=
    // baseline,avx2+fma,avx512,avx512+vnni) fails here if detection drops one.
    if let Some(expected) = std::env::var_os("KAI_EXPECT_TIERS") {
        let expected = expected.to_string_lossy().into_owned();
        assert_eq!(names.join(","), expected, "KAI_EXPECT_TIERS");
    }
}
