//! Q8_0 quantisation and the matrix-vector product the engine spends its time in.
//!
//! Weights stay in the model file's Q8_0 blocks. Each activation vector is
//! quantised to Q8_0 too ([`QVec`]), so a block's dot product is an exact integer
//! sum, and [`matvec`] runs it in the [`Tier`] the CPU picks at runtime. The tiers
//! give the same bits, so a guest's answer does not depend on its CPU.

#![no_std]

extern crate alloc;

mod dot;
pub mod f16;
mod q8_0;

#[cfg(test)]
mod tests;

pub use dot::{Tier, matvec};
pub use q8_0::{BLOCK_BYTES, BLOCK_LEN, QVec, dequantize_row, quantize_row, row_bytes};
