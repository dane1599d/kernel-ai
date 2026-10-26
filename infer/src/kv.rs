//! The f16 KV cache and the rotary-position table, both sized to a context cap
//! chosen when the model is loaded, never to the 40,960 tokens Qwen3 was trained
//! for.

use crate::{Config, Error};
use alloc::vec::Vec;

/// Keys and values of every layer, as f16 halves (rounded to nearest, ties to
/// even, as llama.cpp keeps its default cache), for up to `capacity` positions, laid out
/// `[layer][position][kv_dim]`.
pub struct KvCache {
    keys: Vec<u16>,
    values: Vec<u16>,
    kv_dim: usize,
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
            kv_dim: config.kv_dim(),
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

    /// Forgets the positions from `len` on and keeps those before it: the keys and
    /// values at a position depend only on the tokens there and before, so they
    /// hold for any prompt that starts with those tokens.
    pub fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }

    /// Bytes the cache holds.
    pub fn bytes(&self) -> usize {
        (self.keys.len() + self.values.len()) * core::mem::size_of::<u16>()
    }

    /// Whether the cache was made for a model of this shape.
    pub(crate) fn fits(&self, config: &Config) -> bool {
        self.kv_dim == config.kv_dim()
            && Some(self.keys.len()) == config.n_layer.checked_mul(self.kv_dim * self.capacity)
    }

    pub(crate) fn advance(&mut self, positions: usize) {
        self.len += positions;
    }

    fn range(&self, layer: usize, pos: usize) -> core::ops::Range<usize> {
        let start = (layer * self.capacity + pos) * self.kv_dim;
        start..start + self.kv_dim
    }

    /// The key and value slots of `layer` at `pos`, to be filled.
    pub(crate) fn slot_mut(&mut self, layer: usize, pos: usize) -> (&mut [u16], &mut [u16]) {
        let range = self.range(layer, pos);
        (&mut self.keys[range.clone()], &mut self.values[range])
    }

    /// The layer count and the KV width it was made for.
    pub(crate) fn shape(&self) -> (usize, usize) {
        (self.keys.len() / (self.kv_dim * self.capacity), self.kv_dim)
    }

    /// Keys and values of `layer` for its first `n` positions.
    pub(crate) fn positions(&self, layer: usize, n: usize) -> (&[u16], &[u16]) {
        let start = self.range(layer, 0).start;
        let end = start + n * self.kv_dim;
        (&self.keys[start..end], &self.values[start..end])
    }

    /// The same, to be filled by a restore.
    pub(crate) fn positions_mut(&mut self, layer: usize, n: usize) -> (&mut [u16], &mut [u16]) {
        let start = self.range(layer, 0).start;
        let end = start + n * self.kv_dim;
        (&mut self.keys[start..end], &mut self.values[start..end])
    }

    /// Counts the first `n` positions as filled, once a restore has filled them.
    pub(crate) fn set_len(&mut self, n: usize) {
        self.len = n.min(self.capacity);
    }

    /// Keys and values of `layer` for positions `0..=last`.
    pub(crate) fn layer(&self, layer: usize, last: usize) -> (&[u16], &[u16]) {
        let start = self.range(layer, 0).start;
        let end = self.range(layer, last).end;
        (&self.keys[start..end], &self.values[start..end])
    }
}

fn zeros<T: Copy + Default>(n: usize) -> Result<Vec<T>, Error> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
    v.resize(n, T::default());
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
