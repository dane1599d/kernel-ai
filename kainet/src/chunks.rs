//! `kai-chunks 1` (ADR 0008 §9): the signed list a large download is checked
//! against, chunk by chunk, before a byte of it is written.
//!
//! ```text
//! kai-chunks 1
//! blob <sha256> <size>
//! chunk <bytes>
//! <sha256 of chunk 0>
//! <sha256 of chunk 1>
//! ...
//! ```
//!
//! Hashes are lower-case hex, lines end with LF, there is exactly one hash a
//! chunk (the last may be short), and nothing follows. A chunk is a power of
//! two from 64 KiB to 16 MiB; P5 uses 2 MiB. Anything else is refused: the
//! list comes from signed metadata, but this reader trusts nothing it is
//! given.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

/// P5's chunk: 2 MiB.
pub const CHUNK: u64 = 2 * 1024 * 1024;
/// The smallest and largest chunk a list may name.
pub const MIN_CHUNK: u64 = 64 * 1024;
pub const MAX_CHUNK: u64 = 16 * 1024 * 1024;
/// The most chunks a list may hold: 32 GiB at 2 MiB, and a list of about
/// 1 MiB.
pub const MAX_CHUNKS: u64 = 16 * 1024;
/// The longest list read.
pub const MAX_LIST: usize = 1 << 20;

const HEADER: &str = "kai-chunks 1";

/// A SHA-256.
pub type Hash = [u8; 32];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunks {
    /// The whole target's SHA-256.
    pub blob: Hash,
    pub size: u64,
    pub chunk: u64,
    /// One a chunk.
    pub hashes: Vec<Hash>,
}

impl Chunks {
    /// The list for `blob`, in chunks of `chunk` bytes.
    pub fn of(blob: &[u8], chunk: u64) -> Self {
        let step = usize::try_from(chunk).unwrap_or(usize::MAX);
        Self {
            blob: Sha256::digest(blob).into(),
            size: blob.len() as u64,
            chunk,
            hashes: blob
                .chunks(step.max(1))
                .map(|c| Sha256::digest(c).into())
                .collect(),
        }
    }

    /// How many chunks the target has.
    pub fn count(&self) -> usize {
        self.hashes.len()
    }

    /// Chunk `i`'s bytes in the target, as `[start, end)`.
    pub fn range(&self, i: usize) -> (u64, u64) {
        let start = i as u64 * self.chunk;
        (start, (start + self.chunk).min(self.size))
    }

    /// The list as text.
    pub fn render(&self) -> String {
        let mut out = format!(
            "{HEADER}\nblob {} {}\nchunk {}\n",
            hex(&self.blob),
            self.size,
            self.chunk
        );
        for hash in &self.hashes {
            out.push_str(&hex(hash));
            out.push('\n');
        }
        out
    }

    /// `text` as a list, or why not.
    pub fn parse(text: &[u8]) -> Result<Self, &'static str> {
        if text.len() > MAX_LIST {
            return Err("a kai-chunks list is over 1 MiB");
        }
        let text = core::str::from_utf8(text).map_err(|_| "a kai-chunks list is not text")?;
        let body = text
            .strip_suffix('\n')
            .ok_or("a kai-chunks list ends with a newline")?;
        let mut lines = body.split('\n');
        if lines.next() != Some(HEADER) {
            return Err("not a kai-chunks 1 list");
        }
        let blob_line = lines.next().ok_or("a kai-chunks list has no blob line")?;
        let mut words = blob_line.split(' ');
        let (Some("blob"), Some(blob), Some(size), None) =
            (words.next(), words.next(), words.next(), words.next())
        else {
            return Err("a kai-chunks blob line is `blob <sha256> <size>`");
        };
        let blob = parse_hash(blob)?;
        let size = parse_number(size)?;
        let chunk = lines
            .next()
            .and_then(|l| l.strip_prefix("chunk "))
            .ok_or("a kai-chunks list has no chunk line")?;
        let chunk = parse_number(chunk)?;
        if !chunk.is_power_of_two() || !(MIN_CHUNK..=MAX_CHUNK).contains(&chunk) {
            return Err("a kai-chunks chunk is a power of two from 64 KiB to 16 MiB");
        }
        let count = size.div_ceil(chunk);
        if count > MAX_CHUNKS {
            return Err("a kai-chunks list holds more than 16,384 chunks");
        }
        let mut hashes = Vec::with_capacity(count as usize);
        for line in lines {
            if hashes.len() as u64 == count {
                return Err("a kai-chunks list has more hashes than chunks");
            }
            hashes.push(parse_hash(line)?);
        }
        if (hashes.len() as u64) < count {
            return Err("a kai-chunks list has fewer hashes than chunks");
        }
        Ok(Self {
            blob,
            size,
            chunk,
            hashes,
        })
    }
}

fn hex(hash: &Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// 64 lower-case hex digits.
fn parse_hash(text: &str) -> Result<Hash, &'static str> {
    let bad = "a kai-chunks hash is 64 lower-case hex digits";
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return Err(bad);
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(bad),
    };
    let mut hash = [0u8; 32];
    for (i, pair) in bytes.chunks(2).enumerate() {
        hash[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Ok(hash)
}

/// A decimal number with no sign, no leading zero, at most 19 digits.
fn parse_number(text: &str) -> Result<u64, &'static str> {
    let bad = "a kai-chunks number is decimal, with no leading zero";
    let good = !text.is_empty()
        && text.len() <= 19
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if !good {
        return Err(bad);
    }
    text.parse().map_err(|_| bad)
}
