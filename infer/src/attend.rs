//! Grouped-query attention for one token over the f16 KV cache, spread over the
//! model's workers one KV head at a time: the query heads that share a KV head
//! widen each of its cached keys and values once ([`quant::attn`], in the
//! model's tier). Each head's scores, softmax and weighted sum run in the same
//! order whoever computes them and in every tier, so any number of workers gives
//! the same bits.

use crate::Config;
use quant::{Parallel, Tier, attn, for_each_chunk};

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
    assert!(positions <= context, "attention shape");
    let piece = group * (hd + context);
    let scale = 1.0 / libm::sqrtf(hd as f32);
    let scratch = &mut scratch[..c.n_head_kv * piece];
    for_each_chunk(par, scratch, piece, &|g, piece| {
        let (outs, scores) = piece.split_at_mut(group * hd);
        let (q, at) = (&q[g * group * hd..][..group * hd], g * hd);
        let keys = (&keys[at..], kv_dim);
        attn::scores(tier, q, hd, keys, positions, scale, scores, context);
        for scores in scores.chunks_exact_mut(context) {
            softmax(&mut scores[..positions]);
        }
        attn::mix(
            tier,
            scores,
            context,
            (&values[at..], kv_dim),
            positions,
            hd,
            outs,
        );
    });
    for (g, piece) in scratch.chunks_exact(piece).enumerate() {
        let at = g * group * hd;
        att[at..at + group * hd].copy_from_slice(&piece[..group * hd]);
    }
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
