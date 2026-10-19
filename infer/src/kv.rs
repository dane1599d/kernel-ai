//! The f32 KV cache and the rotary-position table, both sized to a context cap
//! chosen when the model is loaded, never to the 40,960 tokens Qwen3 was trained
//! for.

use crate::{Config, Error};
use alloc::vec::Vec;

/// Keys and values of every layer for up to `capacity` positions, laid out
/// `[layer][position][kv_dim]`.
pub struct KvCache {
    keys: Vec<f32>,
    values: Vec<f32>,
    capacity: usize,
    len: usize,
}

impl KvCache {
    /// Room for `capacity` tokens, at most the model's trained context.
    pub fn new(config: &Config, capacity: usize) -> Result<Self, Error> {
        if capacity == 0 || capacity > config.context_length {
            return Err(Error::Context);
        }
        config.kv_bytes(capacity).ok_or(Error::OutOfMemory)?;
        let n = config.n_layer * config.kv_dim() * capacity;
        Ok(Self {
            keys: zeros(n)?,
            values: zeros(n)?,
            capacity,
            len: 0,
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Positions filled so far; the next token goes at this position.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Forgets every position, for a new conversation.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Bytes the cache holds.
    pub fn bytes(&self) -> usize {
        (self.keys.len() + self.values.len()) * core::mem::size_of::<f32>()
    }
}

fn zeros(n: usize) -> Result<Vec<f32>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
    v.resize(n, 0.0);
    Ok(v)
}

/// cos and sin of every rotation angle, laid out `[position][pair]`. The angles
/// are built as llama.cpp builds them: `theta = pos`, multiplied in f32 by
/// `base^(-2/head_dim)` once per pair. cos and sin come from `libm` and may
/// differ from llama.cpp's C library in the last bit.
pub struct Rope {
    half: usize,
    positions: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Rope {
    pub fn new(config: &Config, positions: usize) -> Result<Self, Error> {
        let half = config.head_dim / 2;
        let n = positions.checked_mul(half).ok_or(Error::OutOfMemory)?;
        let (mut cos, mut sin) = (zeros(n)?, zeros(n)?);
        let scale = libm::powf(config.rope_theta, -2.0 / config.head_dim as f32);
        for pos in 0..positions {
            let mut theta = pos as f32;
            for i in 0..half {
                cos[pos * half + i] = libm::cosf(theta);
                sin[pos * half + i] = libm::sinf(theta);
                theta *= scale;
            }
        }
        Ok(Self {
            half,
            positions,
            cos,
            sin,
        })
    }

    /// Positions the table covers.
    pub fn positions(&self) -> usize {
        self.positions
    }

    /// Rotates one head (exactly `head_dim` values) at `pos`, below
    /// [`Rope::positions`], NeoX style: element `i` pairs with `i + half`.
    pub fn apply(&self, head: &mut [f32], pos: usize) {
        debug_assert_eq!(
            head.len(),
            2 * self.half,
            "rope applied to a slice that is not one head"
        );
        assert!(pos < self.positions, "rope position {pos} past the table");
        let (lo, hi) = head.split_at_mut(self.half);
        let at = pos * self.half;
        let angles = self.cos[at..at + self.half].iter().zip(&self.sin[at..]);
        for ((a, b), (&c, &s)) in lo.iter_mut().zip(hi).zip(angles) {
            let (x0, x1) = (*a, *b);
            *a = x0 * c - x1 * s;
            *b = x0 * s + x1 * c;
        }
    }
}
