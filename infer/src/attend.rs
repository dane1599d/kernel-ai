//! Grouped-query attention for one token over the f16 KV cache, spread over the
//! model's workers one KV head at a time: the query heads that share a KV head
//! widen each of its cached keys and values once. Each head's scores, softmax and
//! weighted sum run in the same order whoever computes them, so any number of
//! workers gives the same bits.

use crate::Config;
use quant::{Parallel, Tier, f16, for_each_chunk};

/// Longest head the stack buffer for one widened key or value row holds:
/// `config::MAX_HEAD_DIM`.
const MAX_HEAD: usize = 1024;

/// Floats of scratch [`attend`] needs for a context of `context` positions: for
/// each query head, its output and its scores.
pub(crate) fn scratch_len(c: &Config, context: usize) -> usize {
    c.n_head * (c.head_dim + context)
}

/// Attention of one token's queries `q` (`q_dim` values) over positions
/// `0..positions` of one layer's `keys` and `values`, into `att` (`q_dim`
/// values). `scratch` holds [`scratch_len`] floats for `context` positions.
#[allow(clippy::too_many_arguments)]
pub(crate) fn attend(
    par: &dyn Parallel,
    tier: Tier,
    c: &Config,
    q: &[f32],
    (keys, values): (&[u16], &[u16]),
    positions: usize,
    (scratch, context): (&mut [f32], usize),
    att: &mut [f32],
) {
    let (hd, kv_dim) = (c.head_dim, c.kv_dim());
    let group = c.n_head / c.n_head_kv;
    assert!(hd <= MAX_HEAD && positions <= context, "attention shape");
    let piece = group * (hd + context);
    let scale = 1.0 / libm::sqrtf(hd as f32);
    let scratch = &mut scratch[..c.n_head_kv * piece];
    for_each_chunk(par, scratch, piece, &|g, piece| {
        let (outs, scores) = piece.split_at_mut(group * hd);
        let mut buffer = [0.0f32; MAX_HEAD];
        let row = &mut buffer[..hd];
        let at = g * hd;
        for t in 0..positions {
            f16::widen(tier, &keys[t * kv_dim + at..][..hd], row);
            for (h, scores) in scores.chunks_exact_mut(context).enumerate() {
                let q = &q[(g * group + h) * hd..][..hd];
                scores[t] = dot(q, row) * scale;
            }
        }
        for scores in scores.chunks_exact_mut(context) {
            softmax(&mut scores[..positions]);
        }
        outs.fill(0.0);
        for t in 0..positions {
            f16::widen(tier, &values[t * kv_dim + at..][..hd], row);
            for (out, scores) in outs.chunks_exact_mut(hd).zip(scores.chunks_exact(context)) {
                let p = scores[t];
                for (o, &v) in out.iter_mut().zip(&*row) {
                    *o += p * v;
                }
            }
        }
    });
    for (g, piece) in scratch.chunks_exact(piece).enumerate() {
        let at = g * group * hd;
        att[at..at + group * hd].copy_from_slice(&piece[..group * hd]);
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

/// Softmax in place: the largest score is subtracted first, and the sum runs in
/// f64 as in llama.cpp.
fn softmax(scores: &mut [f32]) {
    let max = scores.iter().fold(f32::NEG_INFINITY, |m, &v| m.max(v));
    let mut sum = 0.0f64;
    for v in scores.iter_mut() {
        *v = libm::expf(*v - max);
        sum += f64::from(*v);
    }
    let inv = (1.0 / sum) as f32;
    for v in scores.iter_mut() {
        *v *= inv;
    }
}
