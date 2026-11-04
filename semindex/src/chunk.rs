//! A file's text in chunks (ADR 0013 section 4): about [`TARGET`] tokens each,
//! the last [`OVERLAP`] of one shared with the next, so a sentence cut by one
//! chunk is whole in another. Chunks start and end on whitespace, and a word
//! longer than a chunk is cut between characters, never inside one.
//!
//! Tokens are counted by [`Tokens`]: the embedding model's own tokenizer once
//! S13-3 brings one, and until then [`Estimate`], about four bytes a token, as
//! the shell estimates its own budgets.

use alloc::vec::Vec;

/// Tokens a chunk holds at most.
pub const TARGET: usize = 400;
/// Tokens a chunk shares with the one before it.
pub const OVERLAP: usize = 40;
/// Most bytes of one piece of a word too long for a chunk.
pub const MAX_PIECE: usize = 1024;
/// Most chunks one file gives; the rest of a longer file is not indexed.
pub const MAX_CHUNKS: usize = 4096;

/// Counts the tokens of a piece of text.
pub trait Tokens {
    fn count(&self, text: &str) -> usize;
}

/// About four bytes a token, and at least one for any word.
#[derive(Clone, Copy, Debug, Default)]
pub struct Estimate;

impl Tokens for Estimate {
    fn count(&self, text: &str) -> usize {
        text.len().div_ceil(4).max(1)
    }
}

/// One chunk: where it is in the file's text, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub offset: usize,
    pub len: usize,
}

impl Chunk {
    /// The chunk's text in `text`, the text it was cut from.
    pub fn of<'a>(&self, text: &'a str) -> &'a str {
        &text[self.offset..self.offset + self.len]
    }
}

/// A word, or a piece of one too long for a chunk, and its tokens.
struct Word {
    start: usize,
    end: usize,
    tokens: usize,
}

/// The words of `text`: its runs of non-whitespace, a run longer than
/// [`MAX_PIECE`] bytes cut between characters.
fn words(text: &str, tokens: &dyn Tokens) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    let push = |out: &mut Vec<Word>, from: usize, to: usize| {
        let mut at = from;
        while at < to {
            let mut end = (at + MAX_PIECE).min(to);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == at {
                // A character wider than a piece cannot be; take it whole.
                end = at + text[at..].chars().next().map_or(1, char::len_utf8);
            }
            out.push(Word {
                start: at,
                end,
                tokens: tokens.count(&text[at..end]),
            });
            at = end;
        }
    };
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(from)) => {
                push(&mut out, from, i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        push(&mut out, from, text.len());
    }
    out
}

/// `text` in chunks of at most `target` tokens, each but the first starting
/// with the last words of the one before that come to at most `overlap`
/// tokens. A word over `target` tokens is a chunk of its own. Whitespace-only
/// text has no chunks; at most [`MAX_CHUNKS`] are made.
pub fn chunk_with(text: &str, tokens: &dyn Tokens, target: usize, overlap: usize) -> Vec<Chunk> {
    let words = words(text, tokens);
    let mut chunks = Vec::new();
    let mut first = 0;
    while first < words.len() && chunks.len() < MAX_CHUNKS {
        let mut end = first;
        let mut used = 0;
        while end < words.len() && (end == first || used + words[end].tokens <= target) {
            used += words[end].tokens;
            end += 1;
        }
        chunks.push(Chunk {
            offset: words[first].start,
            len: words[end - 1].end - words[first].start,
        });
        if end == words.len() {
            break;
        }
        // The next chunk starts with the last words of this one that fit in
        // the overlap, and always after this one's first, so the cut moves on.
        let mut next = end;
        let mut shared = 0;
        while next > first + 1 && shared + words[next - 1].tokens <= overlap {
            shared += words[next - 1].tokens;
            next -= 1;
        }
        first = next;
    }
    chunks
}

/// `text` in chunks of [`TARGET`] tokens sharing [`OVERLAP`].
pub fn chunk(text: &str, tokens: &dyn Tokens) -> Vec<Chunk> {
    chunk_with(text, tokens, TARGET, OVERLAP)
}
