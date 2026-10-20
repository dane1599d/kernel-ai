//! Byte-level BPE, built from a GGUF file's `tokenizer.ggml.*` arrays.
//!
//! [`Tokenizer::encode`] first splits out user-defined tokens such as `<think>`,
//! leftmost and longest first as Hugging Face does (the same as llama.cpp when no
//! two names overlap, as in Qwen's vocabularies), then cuts the rest into pieces
//! with the qwen2 pretokenizer ([`pretok`]). Token strings use GPT-2's byte alphabet ([`bytes`]),
//! so every token stands for a run of bytes; a piece starts as one token per byte
//! and adjacent pairs merge in the order the merge list ranks them. A control
//! token such as `<|im_start|>` never comes from text: [`Tokenizer::special`]
//! finds it by name, and [`chat`] builds the ChatML prompt with it.

#![no_std]

extern crate alloc;

pub mod bytes;
pub mod chat;
mod marks;
mod merge;
pub mod pretok;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod text_tests;

use alloc::vec::Vec;

/// The most tokens a vocabulary may have; ids must fit in a u32 with room for a
/// marker.
pub const MAX_TOKENS: usize = 1 << 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// `tokenizer.ggml.model` is not `gpt2`, the byte-level BPE this crate runs.
    NotGpt2,
    MissingKey(&'static str),
    /// More than [`MAX_TOKENS`] tokens, or none.
    BadTokenCount,
    /// `token_type` does not have one entry per token.
    TypeCount,
    /// The merge at this rank is not two known tokens joined into a known token.
    BadMerge(usize),
    /// No token stands for this single byte.
    MissingByte(u8),
    /// `tokenizer.ggml.pre` is not `qwen2`, the only pretokenizer here.
    Pretokenizer,
    /// The vocabulary has no token with this text of the kind a chat needs (a
    /// control token for the turn markers).
    MissingSpecial(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::NotGpt2 => f.write_str("the tokenizer is not byte-level BPE (gpt2)"),
            Self::MissingKey(key) => write!(f, "the file has no usable {key}"),
            Self::BadTokenCount => f.write_str("the vocabulary is empty or too large"),
            Self::TypeCount => f.write_str("token types and tokens differ in number"),
            Self::BadMerge(rank) => write!(f, "merge {rank} does not join known tokens"),
            Self::MissingByte(b) => write!(f, "no token stands for byte {b:#04x}"),
            Self::Pretokenizer => f.write_str("the pretokenizer is not qwen2"),
            Self::MissingSpecial(name) => write!(f, "the vocabulary has no {name}"),
        }
    }
}

/// What a token is, from `tokenizer.ggml.token_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Unknown,
    /// Structure the text never contains, such as `<|im_end|>`.
    Control,
    /// Literal text added to the vocabulary, such as `<think>`.
    UserDefined,
    Unused,
    Byte,
}

impl Kind {
    fn from_type(value: i64) -> Self {
        match value {
            2 => Self::Unknown,
            3 => Self::Control,
            4 => Self::UserDefined,
            5 => Self::Unused,
            6 => Self::Byte,
            _ => Self::Normal,
        }
    }

    /// Whether the token's string is in the byte alphabet and can come out of BPE.
    fn is_bpe(self) -> bool {
        matches!(self, Self::Normal | Self::Byte)
    }
}

pub struct Tokenizer {
    /// Every token's bytes back to back: token i is `bytes[starts[i]..starts[i + 1]]`.
    bytes: Vec<u8>,
    starts: Vec<u32>,
    kinds: Vec<Kind>,
    /// BPE tokens sorted by their bytes, lowest id first among equals.
    by_bytes: Vec<u32>,
    /// Control and user-defined tokens sorted by their text.
    specials: Vec<u32>,
    /// User-defined tokens, longest first, which text can spell.
    user_defined: Vec<u32>,
    byte_token: [u32; 256],
    /// `(left << 32 | right, rank, merged)`, sorted by the pair.
    merges: Vec<(u64, u32, u32)>,
}

impl Tokenizer {
    /// Builds the tokenizer from a GGUF header's `tokenizer.ggml.*` entries.
    pub fn from_gguf(header: &gguf::Header<'_>) -> Result<Self, Error> {
        let model = header.get("tokenizer.ggml.model").and_then(|v| v.as_str());
        if model != Some("gpt2") {
            return Err(Error::NotGpt2);
        }
        let pre = header.get("tokenizer.ggml.pre").and_then(|v| v.as_str());
        if pre != Some("qwen2") {
            return Err(Error::Pretokenizer);
        }
        let array = |key: &'static str| {
            header
                .get(key)
                .and_then(|v| v.as_array())
                .ok_or(Error::MissingKey(key))
        };
        let tokens = array("tokenizer.ggml.tokens")?
            .strs()
            .ok_or(Error::MissingKey("tokenizer.ggml.tokens"))?;
        let merges = array("tokenizer.ggml.merges")?
            .strs()
            .ok_or(Error::MissingKey("tokenizer.ggml.merges"))?;
        let types = match header.get("tokenizer.ggml.token_type") {
            None => None,
            Some(value) => Some(
                value
                    .as_array()
                    .and_then(|a| a.ints())
                    .ok_or(Error::MissingKey("tokenizer.ggml.token_type"))?,
            ),
        };
        Self::new(tokens, types, merges)
    }

    /// Builds the tokenizer from token strings, their types (all normal when
    /// `None`) and merges written `left right`, best first.
    pub fn new<'a>(
        tokens: impl ExactSizeIterator<Item = &'a str>,
        types: Option<impl ExactSizeIterator<Item = i64>>,
        merges: impl Iterator<Item = &'a str>,
    ) -> Result<Self, Error> {
        let count = tokens.len();
        if count == 0 || count > MAX_TOKENS {
            return Err(Error::BadTokenCount);
        }
        let kinds: Vec<Kind> = match types {
            Some(types) if types.len() != count => return Err(Error::TypeCount),
            Some(types) => types.map(Kind::from_type).collect(),
            None => alloc::vec![Kind::Normal; count],
        };
        let mut tok = Self {
            bytes: Vec::new(),
            starts: Vec::with_capacity(count + 1),
            kinds,
            by_bytes: Vec::new(),
            specials: Vec::new(),
            user_defined: Vec::new(),
            byte_token: [0; 256],
            merges: Vec::new(),
        };
        tok.starts.push(0);
        for (id, text) in tokens.enumerate() {
            // An iterator longer than it said is refused, not indexed past.
            let kind = *tok.kinds.get(id).ok_or(Error::BadTokenCount)?;
            if kind.is_bpe() {
                decode_alphabet(text, &mut tok.bytes);
            } else {
                tok.bytes.extend_from_slice(text.as_bytes());
            }
            let end = u32::try_from(tok.bytes.len()).map_err(|_| Error::BadTokenCount)?;
            tok.starts.push(end);
        }
        if tok.starts.len() != count + 1 {
            return Err(Error::BadTokenCount);
        }
        tok.index()?;
        tok.load_merges(merges)?;
        Ok(tok)
    }

    fn index(&mut self) -> Result<(), Error> {
        let ids = 0..self.kinds.len() as u32;
        let (mut bpe, mut specials): (Vec<u32>, Vec<u32>) =
            ids.partition(|&id| self.kinds[id as usize].is_bpe());
        specials.retain(|&id| matches!(self.kind(id), Kind::Control | Kind::UserDefined));
        bpe.sort_by(|&a, &b| self.token_bytes(a).cmp(self.token_bytes(b)).then(a.cmp(&b)));
        specials.sort_by(|&a, &b| self.token_bytes(a).cmp(self.token_bytes(b)).then(a.cmp(&b)));
        bpe.shrink_to_fit();
        specials.shrink_to_fit();
        let mut user_defined: Vec<u32> = specials
            .iter()
            .copied()
            .filter(|&id| self.kind(id) == Kind::UserDefined && !self.token_bytes(id).is_empty())
            .filter(|&id| core::str::from_utf8(self.token_bytes(id)).is_ok())
            .collect();
        user_defined.sort_by_key(|&id| core::cmp::Reverse(self.token_bytes(id).len()));
        self.by_bytes = bpe;
        self.specials = specials;
        self.user_defined = user_defined;
        for b in 0..=255u8 {
            self.byte_token[b as usize] = self.find(&[b]).ok_or(Error::MissingByte(b))?;
        }
        Ok(())
    }

    fn load_merges<'a>(&mut self, merges: impl Iterator<Item = &'a str>) -> Result<(), Error> {
        let mut scratch = Vec::new();
        let mut table = Vec::new();
        for (rank, text) in merges.enumerate() {
            let rank32 = u32::try_from(rank).map_err(|_| Error::BadMerge(rank))?;
            let (left, right) = text.split_once(' ').ok_or(Error::BadMerge(rank))?;
            scratch.clear();
            decode_alphabet(left, &mut scratch);
            let split = scratch.len();
            let l = self.find(&scratch);
            decode_alphabet(right, &mut scratch);
            let r = self.find(&scratch[split..]);
            let merged = self.find(&scratch);
            match (l, r, merged) {
                (Some(l), Some(r), Some(m)) if split > 0 && split < scratch.len() => {
                    table.push((u64::from(l) << 32 | u64::from(r), rank32, m));
                }
                _ => return Err(Error::BadMerge(rank)),
            }
        }
        // The best rank wins if a pair is listed twice.
        table.sort_unstable_by_key(|&(pair, rank, _)| (pair, rank));
        table.dedup_by_key(|m| m.0);
        table.shrink_to_fit();
        self.merges = table;
        Ok(())
    }

    /// The BPE token whose bytes are exactly `bytes`.
    fn find(&self, bytes: &[u8]) -> Option<u32> {
        let at = self
            .by_bytes
            .partition_point(|&id| self.token_bytes(id) < bytes);
        let id = *self.by_bytes.get(at)?;
        (self.token_bytes(id) == bytes).then_some(id)
    }

    /// The rank of merging `left` then `right`, and the token it makes.
    fn merge_of(&self, left: u32, right: u32) -> Option<(u32, u32)> {
        let pair = u64::from(left) << 32 | u64::from(right);
        let at = self.merges.binary_search_by_key(&pair, |m| m.0).ok()?;
        let (_, rank, merged) = self.merges[at];
        Some((rank, merged))
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// What `id` is; an id past the vocabulary is unused.
    pub fn kind(&self, id: u32) -> Kind {
        self.kinds.get(id as usize).copied().unwrap_or(Kind::Unused)
    }

    /// The bytes a token stands for, or its text for control and user-defined
    /// tokens; empty past the vocabulary.
    pub fn token_bytes(&self, id: u32) -> &[u8] {
        let id = id as usize;
        match (self.starts.get(id), self.starts.get(id + 1)) {
            (Some(&start), Some(&end)) => &self.bytes[start as usize..end as usize],
            _ => &[],
        }
    }

    /// The control or user-defined token with this exact text.
    pub fn special(&self, text: &str) -> Option<u32> {
        let text = text.as_bytes();
        let at = self
            .specials
            .partition_point(|&id| self.token_bytes(id) < text);
        let id = *self.specials.get(at)?;
        (self.token_bytes(id) == text).then_some(id)
    }

    /// The tokens for `text`. User-defined tokens spelled in it become those
    /// tokens; control tokens spelled in it stay text.
    pub fn encode(&self, text: &str, out: &mut Vec<u32>) {
        // Where each user-defined name next occurs; a name is searched again only
        // once the cursor has passed its last place, so text full of tags stays
        // linear.
        let mut next: Vec<Option<usize>> = Vec::with_capacity(self.user_defined.len());
        for &id in &self.user_defined {
            next.push(text.find(self.user_defined_name(id)));
        }
        let mut from = 0;
        loop {
            let mut best: Option<(usize, usize)> = None;
            for (i, &id) in self.user_defined.iter().enumerate() {
                let name = self.user_defined_name(id);
                if next[i].is_some_and(|at| at < from) {
                    next[i] = text[from..].find(name).map(|at| at + from);
                }
                // Longest first in the list, so the first at a place wins.
                if let Some(at) = next[i]
                    && best.is_none_or(|(b, _)| at < b)
                {
                    best = Some((at, i));
                }
            }
            let Some((at, i)) = best else {
                break;
            };
            let id = self.user_defined[i];
            self.encode_text(&text[from..at], out);
            out.push(id);
            from = at + self.user_defined_name(id).len();
        }
        self.encode_text(&text[from..], out);
    }

    /// A user-defined token's text; `index` keeps only non-empty UTF-8 names.
    fn user_defined_name(&self, id: u32) -> &str {
        core::str::from_utf8(self.token_bytes(id)).unwrap_or("\u{fffd}")
    }

    fn encode_text(&self, text: &str, out: &mut Vec<u32>) {
        for piece in pretok::split(text) {
            self.encode_piece(piece.as_bytes(), out);
        }
    }

    /// The tokens for one piece of bytes, by byte-pair merging alone.
    pub fn encode_piece(&self, piece: &[u8], out: &mut Vec<u32>) {
        merge::merge_piece(self, piece, out);
    }

    /// Appends the bytes `ids` stand for. Control and unused tokens and ids past
    /// the vocabulary add nothing; user-defined tokens add their text.
    pub fn decode(&self, ids: &[u32], out: &mut Vec<u8>) {
        for &id in ids {
            if !matches!(self.kind(id), Kind::Control | Kind::Unused) {
                out.extend_from_slice(self.token_bytes(id));
            }
        }
    }
}

/// The bytes a string in the byte alphabet stands for. A char outside the
/// alphabet, which no real vocabulary has, stands for its own UTF-8.
fn decode_alphabet(text: &str, out: &mut Vec<u8>) {
    for c in text.chars() {
        match bytes::from_char(c) {
            Some(b) => out.push(b),
            None => {
                let mut buf = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
}
