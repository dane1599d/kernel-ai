//! The two halves of attention over an f16 KV cache, for the query heads that
//! share one KV head: [`scores`] (each head's query times every cached key) and
//! [`mix`] (each head's probabilities times every cached value). Each key or
//! value row is widened once for all the heads.
//!
//! Every tier gives the baseline's bits. A score is a dot product in eight
//! running sums (sum i takes the products of lanes i, i + 8, ...) added as
//! `((s0 + s1) + (s2 + s3)) + ((s4 + s5) + (s6 + s7))`, and a mixed value adds
//! `p * v` position by position from zero, each with a separate multiply and
//! add (Rust never fuses them); the AVX2 kernels keep exactly that order, with
//! one 8-lane register per sum vector.

use crate::f16;
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
use crate::tier::Kind;
use crate::tier::Tier;

/// `scores[h * context + t]` = query head h of `q` (heads of `hd` values one
/// after another) times key t, times `scale`, for t in `0..positions`. Key t is
/// `keys[t * stride..][..hd]`.
#[allow(clippy::too_many_arguments)]
pub fn scores(
    tier: Tier,
    q: &[f32],
    hd: usize,
    (keys, stride): (&[u16], usize),
    positions: usize,
    scale: f32,
    scores: &mut [f32],
    context: usize,
) {
    check(
        q.len(),
        hd,
        keys.len(),
        stride,
        positions,
        scores.len(),
        context,
    );
    match tier.0 {
        // SAFETY: every SIMD tier needs AVX2 and F16C with the OS saving ymm
        // state (`tier::kinds`), and holding the tier proves the CPU has them;
        // `check` bounded every read and write.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 | Kind::Avx512 | Kind::Avx512Vnni if hd.is_multiple_of(8) => unsafe {
            crate::attn_avx2::scores(q, hd, keys, stride, positions, scale, scores, context)
        },
        _ => scores_baseline(q, hd, keys, stride, positions, scale, scores, context),
    }
}

/// `out[h * hd..][..hd]` = the sum over t in `0..positions`, in order from zero,
/// of `scores[h * context + t]` times value t, for each head h of `out`. Value t
/// is `values[t * stride..][..hd]`.
#[allow(clippy::too_many_arguments)]
pub fn mix(
    tier: Tier,
    scores: &[f32],
    context: usize,
    (values, stride): (&[u16], usize),
    positions: usize,
    hd: usize,
    out: &mut [f32],
) {
    check(
        out.len(),
        hd,
        values.len(),
        stride,
        positions,
        scores.len(),
        context,
    );
    match tier.0 {
        // SAFETY: as in `scores`.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 | Kind::Avx512 | Kind::Avx512Vnni if hd.is_multiple_of(64) => unsafe {
            crate::attn_avx2::mix(scores, context, values, stride, positions, hd, out)
        },
        _ => mix_baseline(scores, context, values, stride, positions, hd, out),
    }
}

/// Checks that `heads` heads of `hd` values, `positions` rows `stride` apart and
/// a row of `context` scores per head fit their slices.
fn check(
    head_values: usize,
    hd: usize,
    rows_len: usize,
    stride: usize,
    positions: usize,
    scores_len: usize,
    context: usize,
) {
    assert!(hd > 0 && head_values.is_multiple_of(hd), "whole heads");
    let heads = head_values / hd;
    assert!(positions <= context, "positions past the context");
    assert!(
        heads.checked_mul(context).is_some_and(|n| n <= scores_len),
        "a row of scores per head"
    );
    if positions > 0 {
        let last = (positions - 1)
            .checked_mul(stride)
            .and_then(|s| s.checked_add(hd));
        assert!(
            last.is_some_and(|end| end <= rows_len),
            "rows inside the cache"
        );
    }
}

/// Longest row the baseline widens on the stack.
const MAX_HEAD: usize = 1024;

#[allow(clippy::too_many_arguments)]
fn scores_baseline(
    q: &[f32],
    hd: usize,
    keys: &[u16],
    stride: usize,
    positions: usize,
    scale: f32,
    scores: &mut [f32],
    context: usize,
) {
    assert!(hd <= MAX_HEAD, "head too long");
    let mut buffer = [0.0f32; MAX_HEAD];
    let row = &mut buffer[..hd];
    for t in 0..positions {
        f16::widen(Tier::BASELINE, &keys[t * stride..][..hd], row);
        for (q, scores) in q.chunks_exact(hd).zip(scores.chunks_exact_mut(context)) {
            scores[t] = dot(q, row) * scale;
        }
    }
}

fn mix_baseline(
    scores: &[f32],
    context: usize,
    values: &[u16],
    stride: usize,
    positions: usize,
    hd: usize,
    out: &mut [f32],
) {
    assert!(hd <= MAX_HEAD, "head too long");
    let mut buffer = [0.0f32; MAX_HEAD];
    let row = &mut buffer[..hd];
    out.fill(0.0);
    for t in 0..positions {
        f16::widen(Tier::BASELINE, &values[t * stride..][..hd], row);
        for (out, scores) in out.chunks_exact_mut(hd).zip(scores.chunks_exact(context)) {
            let p = scores[t];
            for (o, &v) in out.iter_mut().zip(&*row) {
                *o += p * v;
            }
        }
    }
}

/// A dot product in eight running sums, so it vectorises while its order stays
/// fixed in the source.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut sums = [0.0f32; 8];
    let (a8, a_tail) = a.as_chunks::<8>();
    let (b8, b_tail) = b.as_chunks::<8>();
    for (a, b) in a8.iter().zip(b8) {
        for i in 0..8 {
            sums[i] += a[i] * b[i];
        }
    }
    for (i, (x, y)) in a_tail.iter().zip(b_tail).enumerate() {
        sums[i] += x * y;
    }
    ((sums[0] + sums[1]) + (sums[2] + sums[3])) + ((sums[4] + sums[5]) + (sums[6] + sums[7]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ksupport::Rng;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Halves spread over many exponents, with some zeros, infinities and NaNs.
    fn halves(rng: &mut Rng, len: usize, odd: bool) -> Vec<u16> {
        (0..len)
            .map(|_| match rng.next() % 64 {
                0 if odd => 0x7c00,
                1 if odd => 0x7e01,
                2 => 0x8000,
                _ => f16::from_f32(rng.unit() * 2.0f32.powi(rng.next() as i32 % 12 - 6)),
            })
            .collect()
    }

    fn same(a: &[f32], b: &[f32]) -> bool {
        a.iter()
            .zip(b)
            .all(|(a, b)| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()))
    }

    #[test]
    fn every_tier_gives_the_baselines_bits() {
        let mut rng = Rng(77);
        // Heads 1 to 6 cover each batch size and a batch of 4 then a rest.
        for (heads, hd) in [
            (1, 64),
            (2, 128),
            (3, 64),
            (4, 128),
            (5, 64),
            (6, 128),
            (2, 72),
        ] {
            for positions in [0, 1, 9, 70] {
                for odd in [false, true] {
                    let (stride, context) = (hd * 3 + 8, positions + 5);
                    let rows = halves(&mut rng, positions.max(1) * stride + hd, odd);
                    let rows = &rows[5..];
                    let q: Vec<f32> = (0..heads * hd).map(|_| rng.unit() * 4.0).collect();
                    let p: Vec<f32> = (0..heads * context).map(|_| rng.unit().abs()).collect();
                    let mut want_s = vec![0.5f32; heads * context];
                    let mut want_o = vec![0.5f32; heads * hd];
                    scores(
                        Tier::BASELINE,
                        &q,
                        hd,
                        (rows, stride),
                        positions,
                        0.3,
                        &mut want_s,
                        context,
                    );
                    mix(
                        Tier::BASELINE,
                        &p,
                        context,
                        (rows, stride),
                        positions,
                        hd,
                        &mut want_o,
                    );
                    for tier in Tier::all() {
                        let mut got_s = vec![0.5f32; heads * context];
                        let mut got_o = vec![0.5f32; heads * hd];
                        scores(
                            tier,
                            &q,
                            hd,
                            (rows, stride),
                            positions,
                            0.3,
                            &mut got_s,
                            context,
                        );
                        mix(tier, &p, context, (rows, stride), positions, hd, &mut got_o);
                        let what = (tier.name(), heads, hd, positions, odd);
                        assert!(same(&got_s, &want_s), "scores {what:?}");
                        assert!(same(&got_o, &want_o), "mix {what:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn scores_and_mix_are_dot_products() {
        let mut rng = Rng(5);
        let (hd, positions, context) = (64, 3, 4);
        let rows = halves(&mut rng, positions * hd, false);
        let q: Vec<f32> = (0..2 * hd).map(|_| rng.unit()).collect();
        let p = [0.25f32, 0.5, 0.125, 9.0, 1.0, 0.0, 2.0, 9.0];
        let mut s = vec![0.0f32; 2 * context];
        let mut o = vec![0.0f32; 2 * hd];
        scores(
            Tier::best(),
            &q,
            hd,
            (&rows, hd),
            positions,
            0.5,
            &mut s,
            context,
        );
        mix(
            Tier::best(),
            &p,
            context,
            (&rows, hd),
            positions,
            hd,
            &mut o,
        );
        let v = |t: usize, i: usize| f64::from(f16::to_f32(rows[t * hd + i]));
        for h in 0..2 {
            for t in 0..positions {
                let want: f64 = (0..hd).map(|i| f64::from(q[h * hd + i]) * v(t, i)).sum();
                assert!((f64::from(s[h * context + t]) - want * 0.5).abs() < 1e-4);
            }
            for i in 0..hd {
                let want: f64 = (0..positions)
                    .map(|t| f64::from(p[h * context + t]) * v(t, i))
                    .sum();
                assert!((f64::from(o[h * hd + i]) - want).abs() < 1e-4);
            }
        }
    }

    #[test]
    #[should_panic(expected = "rows inside the cache")]
    fn rows_past_the_cache_are_refused() {
        let rows = vec![0u16; 2 * 64];
        let mut s = vec![0.0f32; 4];
        scores(Tier::best(), &[0.0; 64], 64, (&rows, 64), 3, 1.0, &mut s, 4);
    }
}
