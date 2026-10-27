//! A KV slot's computed positions saved and restored as one file (ADR 0010 §7,
//! S10-3): a cold pack prefill takes 28-41 s in the guest, a restore well under
//! one. The file is `kai-kv 1`:
//!
//! - a [`HEADER`]-byte header, little-endian: the magic; the [`Key`] (model
//!   SHA-256, engine tier, `pack_id`, slot); the model's layer count and KV width;
//!   the positions; the FNV-1a64 of the token ids and of the KV bytes;
//! - the token ids behind the positions, `u32` each;
//! - each layer's keys, then its values, for those positions, `u16` (f16) each.
//!
//! Restore takes nothing on trust. The key, the shape and both hashes must match,
//! and the file's tokens must share a start with the prompt the caller is about
//! to run, before the slot counts a single position as cached; on any mismatch
//! the slot is left empty and the next reply prefills. As with any cached tokens,
//! the next reply then reuses only the positions its prompt shares with them. Bytes move through a small buffer in both
//! directions, so a 150 MiB slot never needs a second copy in memory.

use crate::Error;
use crate::kv::KvCache;
use alloc::vec;
use alloc::vec::Vec;
use quant::Tier;

/// `kai-kv 1`.
pub const MAGIC: [u8; 8] = *b"kai-kv 1";
/// The header's bytes; the rest of it is zero.
pub const HEADER: usize = 128;
/// Bytes moved per step.
const CHUNK: usize = 64 << 10;

/// What a file must have been saved for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    /// The model file's SHA-256.
    pub model_sha256: [u8; 32],
    pub tier: Tier,
    /// The skill pack's id (`llmproto::pack::pack_id`), or 0 for a prompt
    /// without one.
    pub pack_id: u64,
    /// The engine's KV slot.
    pub slot: u8,
}

/// Why a file was not restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not a `kai-kv 1` file.
    Magic,
    /// Saved for another model, tier, pack or slot.
    Key,
    /// Saved for another layer count or KV width, or for more positions than
    /// the slot holds.
    Shape,
    /// Its tokens do not start the prompt, or do not match their hash.
    Tokens,
    /// Its keys and values do not match their hash.
    Values,
    /// The reader ran out of bytes.
    Short,
}

/// FNV-1a 64, as `kabi::model::Fnv1a`.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// A tier's name in the header's 16 bytes.
fn tier_bytes(tier: Tier) -> [u8; 16] {
    let mut out = [0; 16];
    let name = tier.name().as_bytes();
    out[..name.len().min(16)].copy_from_slice(&name[..name.len().min(16)]);
    out
}

fn tokens_hash(tokens: &[u32]) -> u64 {
    let mut fnv = Fnv::new();
    for token in tokens {
        fnv.update(&token.to_le_bytes());
    }
    fnv.0
}

/// The file's length for `positions` of `kv`.
pub fn file_bytes(kv: &KvCache, positions: usize) -> usize {
    let (layers, width) = kv.shape();
    HEADER + 4 * positions + 2 * 2 * layers * width * positions
}

fn header(key: &Key, kv: &KvCache, tokens: &[u32], values_hash: u64) -> [u8; HEADER] {
    let (layers, width) = kv.shape();
    let mut h = [0u8; HEADER];
    h[..8].copy_from_slice(&MAGIC);
    h[8..40].copy_from_slice(&key.model_sha256);
    h[40..56].copy_from_slice(&tier_bytes(key.tier));
    h[56..64].copy_from_slice(&key.pack_id.to_le_bytes());
    h[64] = key.slot;
    h[68..72].copy_from_slice(&(layers as u32).to_le_bytes());
    h[72..76].copy_from_slice(&(width as u32).to_le_bytes());
    h[76..80].copy_from_slice(&(tokens.len() as u32).to_le_bytes());
    h[80..88].copy_from_slice(&tokens_hash(tokens).to_le_bytes());
    h[88..96].copy_from_slice(&values_hash.to_le_bytes());
    h
}

/// Each layer's keys then its values for the first `positions`, in file order.
fn rows(kv: &KvCache, positions: usize) -> impl Iterator<Item = &[u16]> {
    let (layers, _) = kv.shape();
    (0..layers).flat_map(move |layer| {
        let (keys, values) = kv.positions(layer, positions);
        [keys, values]
    })
}

/// Little-endian bytes of `halves`, a chunk at a time, to `out`.
fn each_chunk(halves: &[u16], buf: &mut [u8], out: &mut dyn FnMut(&[u8]) -> bool) -> bool {
    halves.chunks(buf.len() / 2).all(|part| {
        for (pair, h) in buf.as_chunks_mut::<2>().0.iter_mut().zip(part) {
            pair.copy_from_slice(&h.to_le_bytes());
        }
        out(&buf[..2 * part.len()])
    })
}

/// Writes the first `tokens.len()` positions of `kv`, which `tokens` produced,
/// as a `kai-kv 1` file to `out`, in order. `out` returning false stops it: the
/// file is then incomplete, and a restore refuses it.
pub(crate) fn save(
    key: &Key,
    kv: &KvCache,
    tokens: &[u32],
    out: &mut dyn FnMut(&[u8]) -> bool,
) -> Result<(), Error> {
    let n = tokens.len().min(kv.len());
    let tokens = &tokens[..n];
    let mut buf = vec![0u8; CHUNK];
    // The values' hash first, so the header can lead.
    let mut fnv = Fnv::new();
    for row in rows(kv, n) {
        each_chunk(row, &mut buf, &mut |bytes| {
            fnv.update(bytes);
            true
        });
    }
    let mut ok = out(&header(key, kv, tokens, fnv.0));
    let ids: Vec<u8> = tokens.iter().flat_map(|t| t.to_le_bytes()).collect();
    ok = ok && ids.chunks(CHUNK).all(&mut *out);
    ok = ok && rows(kv, n).all(|row| each_chunk(row, &mut buf, out));
    match ok {
        true => Ok(()),
        false => Err(Error::Shape("the KV file's writer stopped")),
    }
}

/// Reads a `kai-kv 1` file from `input` into `kv`, emptied first, for a prompt
/// `prompt`: the tokens restored, which the caller then counts as cached (a
/// reply reuses those its prompt shares). On any refusal `kv` stays empty.
/// `input` fills the buffer it is given, in order, or returns false.
pub(crate) fn restore(
    key: &Key,
    kv: &mut KvCache,
    prompt: &[u32],
    input: &mut dyn FnMut(&mut [u8]) -> bool,
) -> Result<Vec<u32>, Refused> {
    kv.clear();
    let mut h = [0u8; HEADER];
    if !input(&mut h) {
        return Err(Refused::Short);
    }
    if h[..8] != MAGIC {
        return Err(Refused::Magic);
    }
    let u32_at = |at: usize| u32::from_le_bytes(h[at..at + 4].try_into().unwrap()) as usize;
    let u64_at = |at: usize| u64::from_le_bytes(h[at..at + 8].try_into().unwrap());
    if h[8..40] != key.model_sha256
        || h[40..56] != tier_bytes(key.tier)
        || u64_at(56) != key.pack_id
        || h[64] != key.slot
    {
        return Err(Refused::Key);
    }
    let (layers, width) = kv.shape();
    let n = u32_at(76);
    if u32_at(68) != layers || u32_at(72) != width || n > kv.capacity() || n == 0 {
        return Err(Refused::Shape);
    }
    let mut ids = vec![0u8; 4 * n];
    if !input(&mut ids) {
        return Err(Refused::Short);
    }
    let tokens: Vec<u32> = ids
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect();
    // A prompt reuses at most all but its last token (the reply needs its
    // logits), as `Engine::reply` does; a file that gives it none is refused
    // before its values are read.
    let shared = tokens
        .iter()
        .zip(prompt)
        .take_while(|(a, b)| a == b)
        .count();
    if tokens_hash(&tokens) != u64_at(80) || shared.min(prompt.len().saturating_sub(1)) == 0 {
        return Err(Refused::Tokens);
    }
    let mut buf = vec![0u8; CHUNK];
    let mut fnv = Fnv::new();
    for layer in 0..layers {
        let (keys, values) = kv.positions_mut(layer, n);
        for halves in [keys, values] {
            for part in halves.chunks_mut(CHUNK / 2) {
                let bytes = &mut buf[..2 * part.len()];
                if !input(bytes) {
                    return Err(Refused::Short);
                }
                fnv.update(bytes);
                for (h, pair) in part.iter_mut().zip(bytes.as_chunks::<2>().0) {
                    *h = u16::from_le_bytes(*pair);
                }
            }
        }
    }
    if fnv.0 != u64_at(88) {
        return Err(Refused::Values);
    }
    kv.set_len(n);
    Ok(tokens)
}
