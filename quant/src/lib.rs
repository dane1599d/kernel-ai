//! Quantised weights and the matrix-vector products the engine spends its time in.
//!
//! Weights stay in the model file's blocks: Q8_0, or the K-quants Q4_K and Q6_K.
//! Each activation vector is quantised too, to Q8_0 ([`QVec`]) or to 256-value
//! super-blocks ([`Q8KVec`]), so a block's dot product is an exact integer sum,
//! and the products run in the [`Tier`] the CPU picks at runtime. The tiers give
//! the same bits, so a guest's answer does not depend on its CPU. A [`Product`]
//! runs any range of a matrix's rows against one token's vector or a batch of
//! them (the batched kernels unpack each block once for several tokens), and
//! [`matvec_parallel`] spreads the rows over a [`Parallel`]'s workers with the
//! same bits as one thread.

#![no_std]

extern crate alloc;

mod batch;
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
mod batch_avx2;
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
mod batch_avx512;
mod dot;
pub mod f16;
mod kquant;
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
mod kquant_avx2;
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
mod kquant_avx512;
mod parallel;
pub mod q4_k;
pub mod q6_k;
mod q8_0;
mod q8k;
pub mod stream;
mod tier;

#[cfg(test)]
mod batch_tests;
#[cfg(test)]
mod ksupport;
#[cfg(test)]
mod ktests;
#[cfg(test)]
mod parallel_tests;
#[cfg(test)]
mod tests;

pub use batch::{GROUP, q4_k_matmul, q6_k_matmul, q8_0_matmul};
pub use dot::matvec;
pub use kquant::{q4_k_matvec, q6_k_matvec};
pub use parallel::{MAX_PRODUCTS, Parallel, Product, Serial, for_each_chunk, matvec_parallel};
pub use q8_0::{BLOCK_BYTES, BLOCK_LEN, QVec, dequantize_row, quantize_row, row_bytes};
pub use q8k::{Q8KVec, SUPER_LEN};
pub use tier::Tier;
