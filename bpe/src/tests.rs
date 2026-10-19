//! BPE on synthetic vocabularies whose ids are known by construction.

use crate::*;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

const SPECIALS: [(&str, i64); 6] = [
    ("<|endoftext|>", 3),
    ("<|im_start|>", 3),
    ("<|im_end|>", 3),
    ("<think>", 4),
    ("</think>", 4),
    ("[PAD999]", 5),
];

/// The byte alphabet's string for some text.
fn alpha(text: &[u8]) -> String {
    text.iter().map(|&b| bytes::to_char(b)).collect()
}

/// Ids 0-255 are the bytes, then one token per merge in order, then the specials.
fn vocab(merges: &[&str]) -> (Vec<String>, Vec<i64>, Vec<String>) {
    let mut tokens: Vec<String> = (0..=255u8).map(|b| alpha(&[b])).collect();
    let merges: Vec<String> = merges.iter().map(|m| m.to_string()).collect();
    for m in &merges {
        let joined = m.replace(' ', "");
        if !tokens.contains(&joined) {
            tokens.push(joined);
        }
    }
    let mut types = vec![1; tokens.len()];
    for (text, ty) in SPECIALS {
        tokens.push(text.to_string());
        types.push(ty);
    }
    (tokens, types, merges)
}

fn build(merges: &[&str]) -> Tokenizer {
    let (tokens, types, merges) = vocab(merges);
    Tokenizer::new(
        tokens.iter().map(String::as_str),
        Some(types.into_iter()),
        merges.iter().map(String::as_str),
    )
    .unwrap()
}

fn encode(tok: &Tokenizer, piece: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    tok.encode_piece(piece, &mut out);
    out
}

fn text_of(tok: &Tokenizer, ids: &[u32]) -> Vec<Vec<u8>> {
    ids.iter().map(|&id| tok.token_bytes(id).to_vec()).collect()
}

#[test]
fn bytes_without_merges_are_their_own_tokens() {
    let tok = build(&[]);
    let all: Vec<u8> = (0..=255).collect();
    let ids = encode(&tok, &all);
    assert_eq!(ids, (0..256).collect::<Vec<u32>>());
    let mut back = Vec::new();
    tok.decode(&ids, &mut back);
    assert_eq!(back, all);
    assert!(encode(&tok, b"").is_empty());
}

#[test]
fn merges_build_words() {
    let tok = build(&[
        "h e",
        "l l",
        "he ll",
        "hell o",
        "\u{120} w",
        "o r",
        "\u{120}w or",
    ]);
    let hello = encode(&tok, b"hello");
    assert_eq!(text_of(&tok, &hello), [b"hello".to_vec()]);
    let ids = encode(&tok, b" world");
    assert_eq!(
        text_of(&tok, &ids),
        [b" wor".to_vec(), b"l".to_vec(), b"d".to_vec()]
    );
    assert_eq!(hello, [259]);
}

#[test]
fn lower_ranks_merge_first() {
    let tok = build(&["a b", "b c"]);
    assert_eq!(
        text_of(&tok, &encode(&tok, b"abc")),
        [b"ab".to_vec(), b"c".to_vec()]
    );
    let tok = build(&["b c", "a b"]);
    assert_eq!(
        text_of(&tok, &encode(&tok, b"abc")),
        [b"a".to_vec(), b"bc".to_vec()]
    );
}

#[test]
fn equal_ranks_merge_leftmost_first() {
    let tok = build(&["a a"]);
    assert_eq!(
        text_of(&tok, &encode(&tok, b"aaa")),
        [b"aa".to_vec(), b"a".to_vec()]
    );
    let tok = build(&["a a", "aa aa"]);
    assert_eq!(
        text_of(&tok, &encode(&tok, b"aaaaa")),
        [b"aaaa".to_vec(), b"a".to_vec()]
    );
}

#[test]
fn a_long_piece_merges_fully() {
    // 100,001 bytes: the naive way takes seconds here, the heap milliseconds.
    let tok = build(&["= =", "== ==", "==== ===="]);
    let ids = encode(&tok, &[b'='; 100_001]);
    let eight = tok.find(b"========").unwrap();
    assert_eq!(ids.len(), 100_000 / 8 + 1);
    assert!(ids[..ids.len() - 1].iter().all(|&id| id == eight));
    assert_eq!(ids.last(), Some(&u32::from(b'=')));
}

/// Merging the classic way: find the best pair, merge it, start again.
fn naive(tok: &Tokenizer, piece: &[u8]) -> Vec<u32> {
    let mut ids: Vec<u32> = piece.iter().map(|&b| tok.byte_token[b as usize]).collect();
    loop {
        let best = (0..ids.len().saturating_sub(1))
            .filter_map(|i| {
                tok.merge_of(ids[i], ids[i + 1])
                    .map(|(rank, m)| (rank, i, m))
            })
            .min();
        let Some((_, i, merged)) = best else {
            return ids;
        };
        ids[i] = merged;
        ids.remove(i + 1);
    }
}

#[test]
fn heap_merging_matches_the_naive_way() {
    let merges = [
        "a b", "b a", "a a", "ab a", "b b", "ba b", "aa b", "ab ab", "c a", "ca b", "a c", "bb a",
        "aba b", "c c", "cc a", "b c",
    ];
    let tok = build(&merges);
    let mut state = 7u32;
    for len in 0..300 {
        let piece: Vec<u8> = (0..len % 40)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                b"abc"[(state >> 16) as usize % 3]
            })
            .collect();
        assert_eq!(encode(&tok, &piece), naive(&tok, &piece), "{piece:?}");
        let mut back = Vec::new();
        tok.decode(&encode(&tok, &piece), &mut back);
        assert_eq!(back, piece);
    }
}

#[test]
fn specials_are_found_by_name_and_never_made_from_text() {
    let tok = build(&["< |", "i m", "<| im"]);
    let start = tok.special("<|im_start|>").unwrap();
    assert_eq!(tok.kind(start), Kind::Control);
    let think = tok.special("<think>").unwrap();
    assert_eq!(tok.kind(think), Kind::UserDefined);
    assert_eq!(tok.special("hello"), None);
    assert_eq!(tok.special("<|im"), None);
    let ids = encode(&tok, b"<|im_start|>");
    assert!(!ids.contains(&start));
    // Control tokens decode to nothing, user-defined ones to their text.
    let mut out = Vec::new();
    tok.decode(&[start, think, b'x' as u32, 1_000_000], &mut out);
    assert_eq!(out, b"<think>x");
    assert_eq!(tok.token_bytes(start), b"<|im_start|>");
    assert_eq!(tok.token_bytes(u32::MAX), b"");
    assert_eq!(tok.kind(u32::MAX), Kind::Unused);
    // An unused token in the vocabulary is not special and decodes to nothing.
    let pad = (0..tok.len() as u32)
        .find(|&id| tok.token_bytes(id) == b"[PAD999]")
        .unwrap();
    assert_eq!(tok.kind(pad), Kind::Unused);
    assert_eq!(tok.special("[PAD999]"), None);
    let mut out = Vec::new();
    tok.decode(&[pad, b'y' as u32], &mut out);
    assert_eq!(out, b"y");
}

#[test]
fn a_control_token_never_stands_in_for_text() {
    // A control token with the same bytes as a normal one, and a lower id.
    let mut tokens: Vec<String> = (0..=255u8).map(|b| alpha(&[b])).collect();
    tokens.push("ab".into());
    tokens.push("ab".into());
    let mut types = vec![1i64; 256];
    types.extend([3, 1]);
    let tok = Tokenizer::new(
        tokens.iter().map(String::as_str),
        Some(types.into_iter()),
        ["a b"].into_iter(),
    )
    .unwrap();
    assert_eq!(encode(&tok, b"ab"), [257]);
    assert_eq!(tok.special("ab"), Some(256));
}

#[test]
fn the_best_rank_wins_for_a_pair_listed_twice() {
    let tok = build(&["x y", "a b", "x y"]);
    assert_eq!(tok.merge_of(b'x' as u32, b'y' as u32).unwrap().0, 0);
}

#[test]
fn bad_vocabularies_are_refused() {
    let (tokens, types, _) = vocab(&[]);
    let tokens: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let bad_merge = |merge: &str| {
        Tokenizer::new(
            tokens.iter().copied(),
            Some(types.clone().into_iter()),
            [merge].into_iter(),
        )
        .err()
    };
    assert_eq!(bad_merge("ab"), Some(Error::BadMerge(0)));
    assert_eq!(bad_merge("a b"), Some(Error::BadMerge(0)));
    assert_eq!(bad_merge(" a"), Some(Error::BadMerge(0)));
    assert_eq!(bad_merge("a \u{4e2d}"), Some(Error::BadMerge(0)));
    let short = &tokens[1..];
    let result = Tokenizer::new(
        short.iter().copied(),
        None::<core::iter::Empty<i64>>,
        [].into_iter(),
    );
    assert_eq!(result.err(), Some(Error::MissingByte(0)));
    let result = Tokenizer::new(
        tokens.iter().copied(),
        Some([1i64].into_iter()),
        [].into_iter(),
    );
    assert_eq!(result.err(), Some(Error::TypeCount));
    let result = Tokenizer::new(
        [].into_iter(),
        None::<core::iter::Empty<i64>>,
        [].into_iter(),
    );
    assert_eq!(result.err(), Some(Error::BadTokenCount));
    let too_many = core::iter::repeat_n("a", MAX_TOKENS + 1);
    let result = Tokenizer::new(too_many, None::<core::iter::Empty<i64>>, [].into_iter());
    assert_eq!(result.err(), Some(Error::BadTokenCount));
    // An empty token cannot be half of a merge.
    let mut with_empty = tokens.clone();
    with_empty.push("");
    let result = Tokenizer::new(
        with_empty.iter().copied(),
        None::<core::iter::Empty<i64>>,
        [" a"].into_iter(),
    );
    assert_eq!(result.err(), Some(Error::BadMerge(0)));
    // An iterator that says it is shorter than it is.
    let liar = Liar(tokens.iter().copied(), 10);
    let result = Tokenizer::new(liar, None::<core::iter::Empty<i64>>, [].into_iter());
    assert_eq!(result.err(), Some(Error::BadTokenCount));
}

/// Yields everything its inner iterator does but claims a fixed length.
struct Liar<I>(I, usize);

impl<I: Iterator> Iterator for Liar<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.1, Some(self.1))
    }
}

impl<I: Iterator> ExactSizeIterator for Liar<I> {}

#[test]
fn builds_from_a_gguf_header() {
    let (tokens, types, merges) = vocab(&["h i"]);
    let types: Vec<i32> = types.iter().map(|&t| t as i32).collect();
    let mut w = gguf::write::Writer::new();
    w.string("tokenizer.ggml.model", "gpt2")
        .strings("tokenizer.ggml.tokens", &tokens)
        .i32s("tokenizer.ggml.token_type", &types)
        .strings("tokenizer.ggml.merges", &merges);
    let file = w.finish();
    let header = gguf::Header::parse(&file).unwrap();
    let tok = Tokenizer::from_gguf(&header).unwrap();
    assert_eq!(tok.len(), tokens.len());
    assert_eq!(text_of(&tok, &encode(&tok, b"hi")), [b"hi".to_vec()]);
    assert_eq!(tok.kind(tok.special("<|im_end|>").unwrap()), Kind::Control);

    let mut w = gguf::write::Writer::new();
    w.string("tokenizer.ggml.model", "llama")
        .strings("tokenizer.ggml.tokens", &tokens);
    let file = w.finish();
    let header = gguf::Header::parse(&file).unwrap();
    assert_eq!(Tokenizer::from_gguf(&header).err(), Some(Error::NotGpt2));

    let mut w = gguf::write::Writer::new();
    w.string("tokenizer.ggml.model", "gpt2")
        .strings("tokenizer.ggml.tokens", &tokens)
        .string("tokenizer.ggml.merges", "h i");
    let file = w.finish();
    let header = gguf::Header::parse(&file).unwrap();
    let missing = Error::MissingKey("tokenizer.ggml.merges");
    assert_eq!(Tokenizer::from_gguf(&header).err(), Some(missing));

    // Tokens that are not strings, types that are not integers, and no types.
    let mut w = gguf::write::Writer::new();
    w.string("tokenizer.ggml.model", "gpt2")
        .i32s("tokenizer.ggml.tokens", &[1, 2])
        .strings("tokenizer.ggml.merges", &merges);
    let file = w.finish();
    let header = gguf::Header::parse(&file).unwrap();
    let missing = Error::MissingKey("tokenizer.ggml.tokens");
    assert_eq!(Tokenizer::from_gguf(&header).err(), Some(missing));
    let mut w = gguf::write::Writer::new();
    w.string("tokenizer.ggml.model", "gpt2")
        .strings("tokenizer.ggml.tokens", &tokens)
        .strings("tokenizer.ggml.token_type", &tokens)
        .strings("tokenizer.ggml.merges", &merges);
    let file = w.finish();
    let header = gguf::Header::parse(&file).unwrap();
    let missing = Error::MissingKey("tokenizer.ggml.token_type");
    assert_eq!(Tokenizer::from_gguf(&header).err(), Some(missing));
    let mut w = gguf::write::Writer::new();
    w.string("tokenizer.ggml.model", "gpt2")
        .strings("tokenizer.ggml.tokens", &tokens)
        .strings("tokenizer.ggml.merges", &merges);
    let file = w.finish();
    let header = gguf::Header::parse(&file).unwrap();
    let tok = Tokenizer::from_gguf(&header).unwrap();
    // With no types every token is normal, the specials included.
    assert_eq!(tok.special("<|im_end|>"), None);
    assert_eq!(text_of(&tok, &encode(&tok, b"hi")), [b"hi".to_vec()]);
}
