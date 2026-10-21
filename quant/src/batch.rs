//! Matrix products with several tokens' activations at once, for batched
//! prefill: [`q8_0_matmul`], [`q4_k_matmul`] and [`q6_k_matmul`].
//!
//! Running the one-token kernel once per token unpacks every block's scales and
//! codes again for each token. These kernels take one weight row and a group of
//! up to [`GROUP`] tokens, unpack each block once and multiply it by every
//! token's bytes. Per token they compute the same exact integers as the
//! one-token kernels and hand them to the same float tail in the same block
//! order, so each result is the bits of the one-token product in every tier.

use crate::kquant::{Sink, Total, canonical};
use crate::q8_0::{QVec, row_bytes};
use crate::q8k::Q8KVec;
use crate::tier::{Kind, Tier};
use crate::{dot, q4_k, q6_k};

/// Tokens a kernel takes at once: each token keeps an accumulator in a register
/// while a block's unpacked codes stay in others.
pub const GROUP: usize = 8;

/// Runs `$body` on each group of at most [`GROUP`] tokens (`$gx`) and their
/// outputs (`$gy`), `$gx` as an array of its own length so the kernels' token
/// loops unroll.
macro_rules! groups {
    ($xs:expr, $y:expr, |$gx:ident, $gy:ident| $body:block) => {
        for (xs, y) in $xs.chunks(GROUP).zip($y.chunks_mut(GROUP)) {
            groups!(@arms xs, y, $gx, $gy, $body, 8 7 6 5 4 3 2 1);
        }
    };
    (@arms $xs:ident, $y:ident, $gx:ident, $gy:ident, $body:block, $($n:literal)*) => {
        match $xs.len() {
            $($n => {
                let ($gx, $gy): (&[_; $n], &mut [f32]) = ($xs.try_into().unwrap(), $y);
                $body
            })*
            _ => unreachable!("a group of 1 to {GROUP} tokens"),
        }
    };
}

/// `y[r * tokens + t]` = row `r` of the Q8_0 matrix `w` times `xs[t]`, for every
/// row and token, the bits [`crate::matvec`] gives each token. `w` holds
/// `y.len() / xs.len()` rows of `xs[t].len()` values.
pub fn q8_0_matmul(tier: Tier, w: &[u8], xs: &[QVec], y: &mut [f32]) {
    let row = row_bytes(shape(xs.iter().map(QVec::len), w, y, row_bytes));
    for_rows(w, row, xs.len(), y, |w, y| {
        groups!(xs, y, |xs, y| {
            let totals = q8_0_n(tier, w, xs);
            for (y, total) in y.iter_mut().zip(totals) {
                *y = canonical(total);
            }
        })
    });
}

/// As [`q8_0_matmul`], for Q4_K weights: the bits [`crate::q4_k_matvec`] gives
/// each token.
pub fn q4_k_matmul(tier: Tier, w: &[u8], xs: &[Q8KVec], y: &mut [f32]) {
    let row = q4_k::row_bytes(shape(xs.iter().map(Q8KVec::len), w, y, q4_k::row_bytes));
    for_rows(w, row, xs.len(), y, |w, y| {
        groups!(xs, y, |xs, y| {
            let mut totals = xs.each_ref().map(|_| Total(0.0));
            q4_k_n(tier, w, xs, &mut totals);
            for (y, total) in y.iter_mut().zip(totals) {
                *y = canonical(total.0);
            }
        })
    });
}

/// As [`q8_0_matmul`], for Q6_K weights: the bits [`crate::q6_k_matvec`] gives
/// each token.
pub fn q6_k_matmul(tier: Tier, w: &[u8], xs: &[Q8KVec], y: &mut [f32]) {
    let row = q6_k::row_bytes(shape(xs.iter().map(Q8KVec::len), w, y, q6_k::row_bytes));
    for_rows(w, row, xs.len(), y, |w, y| {
        groups!(xs, y, |xs, y| {
            let mut totals = xs.each_ref().map(|_| Total(0.0));
            q6_k_n(tier, w, xs, &mut totals);
            for (y, total) in y.iter_mut().zip(totals) {
                *y = canonical(total.0);
            }
        })
    });
}

/// The one length every vector has (at least one vector), checked against the
/// weights and the output: `w` must hold exactly `y.len() / tokens` rows.
fn shape(
    mut lens: impl Iterator<Item = usize>,
    w: &[u8],
    y: &[f32],
    row_bytes: fn(usize) -> usize,
) -> usize {
    let cols = lens.next().expect("a product needs at least one vector");
    let mut tokens = 1;
    for len in lens {
        assert_eq!(len, cols, "vectors of different lengths");
        tokens += 1;
    }
    assert!(y.len().is_multiple_of(tokens), "output size");
    let rows = y.len() / tokens;
    assert_eq!(
        Some(w.len()),
        row_bytes(cols).checked_mul(rows),
        "matmul shape"
    );
    cols
}

/// Calls `f(row, y)` for each row of `w` and its `tokens` outputs; a matrix of
/// empty rows gives zeros.
fn for_rows(
    w: &[u8],
    row: usize,
    tokens: usize,
    y: &mut [f32],
    mut f: impl FnMut(&[u8], &mut [f32]),
) {
    if row == 0 {
        y.fill(0.0);
        return;
    }
    for (w, y) in w.chunks_exact(row).zip(y.chunks_exact_mut(tokens)) {
        f(w, y);
    }
}

/// One Q8_0 row against each of `xs` in `tier`.
fn q8_0_n<const N: usize>(tier: Tier, row: &[u8], xs: &[QVec; N]) -> [f32; N] {
    match tier.0 {
        Kind::Baseline => dot::dot_baseline_n(row, xs),
        // SAFETY: every one of these tiers exists only when the CPU has AVX2 and
        // FMA (cpufeat's CPUID bits) and the OS saves ymm state (XCR0); see
        // `tier::kinds`. The AVX-512 tiers run the AVX2 kernel for Q8_0, which
        // measured faster than a 512-bit one, as for one token.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 | Kind::Avx512 | Kind::Avx512Vnni => unsafe { dot::avx2::dot_n(row, xs) },
    }
}

/// One Q4_K row against each of `xs` in `tier`, each token's super-blocks handed
/// to its sink.
pub(crate) fn q4_k_n<const N: usize, S: Sink>(
    tier: Tier,
    row: &[u8],
    xs: &[Q8KVec; N],
    sinks: &mut [S; N],
) {
    match tier.0 {
        Kind::Baseline => q4_k::dot_baseline_n(row, xs, sinks),
        // SAFETY: a tier of each kind exists only when the CPU has the features
        // its kernel enables (cpufeat's CPUID bits: AVX2, FMA and F16C; then
        // AVX-512 F, BW and VL; then VNNI) and the OS saves the registers they
        // use (ymm, then zmm and opmask, in XCR0); see `tier::kinds`.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 => unsafe { crate::batch_avx2::q4_k_dot_n(row, xs, sinks) },
        // SAFETY: as above, AVX-512 F, BW and VL.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx512 => unsafe { crate::batch_avx512::q4_k_dot_n(row, xs, sinks) },
        // SAFETY: as above, with AVX-512 VNNI.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx512Vnni => unsafe { crate::batch_avx512::q4_k_dot_n_vnni(row, xs, sinks) },
    }
}

/// One Q6_K row against each of `xs` in `tier`, as [`q4_k_n`] runs Q4_K.
pub(crate) fn q6_k_n<const N: usize, S: Sink>(
    tier: Tier,
    row: &[u8],
    xs: &[Q8KVec; N],
    sinks: &mut [S; N],
) {
    match tier.0 {
        Kind::Baseline => q6_k::dot_baseline_n(row, xs, sinks),
        // SAFETY: as in `q4_k_n`: AVX2, FMA and F16C with ymm state.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 => unsafe { crate::batch_avx2::q6_k_dot_n(row, xs, sinks) },
        // SAFETY: as in `q4_k_n`: AVX-512 F, BW and VL with zmm and opmask state.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx512 => unsafe { crate::batch_avx512::q6_k_dot_n(row, xs, sinks) },
        // SAFETY: as in `q4_k_n`: with AVX-512 VNNI.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx512Vnni => unsafe { crate::batch_avx512::q6_k_dot_n_vnni(row, xs, sinks) },
    }
}
