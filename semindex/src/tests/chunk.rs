//! The chunker: sizes, the overlap, whole words, every word in some chunk.

use crate::chunk::{Chunk, Estimate, MAX_PIECE, OVERLAP, TARGET, Tokens, chunk, chunk_with};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A word a token, so sizes are easy to read.
struct Words;

impl Tokens for Words {
    fn count(&self, _text: &str) -> usize {
        1
    }
}

/// `n` numbered words, `w0 w1 ...`, on lines of ten.
fn numbered(n: usize) -> String {
    let mut text = String::new();
    for i in 0..n {
        text.push_str(&format!("w{i}"));
        text.push(if i % 10 == 9 { '\n' } else { ' ' });
    }
    text
}

fn words_of(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

#[test]
fn whitespace_has_no_chunks_and_a_short_text_is_one_chunk_without_its_edges() {
    assert!(chunk("", &Estimate).is_empty());
    assert!(chunk(" \n\t ", &Estimate).is_empty());
    let text = "  Milk, bread and eggs.\n";
    let chunks = chunk(text, &Estimate);
    assert_eq!(chunks, [Chunk { offset: 2, len: 21 }]);
    assert_eq!(chunks[0].of(text), "Milk, bread and eggs.");
}

#[test]
fn chunks_hold_at_most_the_target_and_share_the_overlap() {
    let text = numbered(1000);
    let chunks = chunk_with(&text, &Words, 400, 40);
    // 400 words, then 360 new words a chunk after the 40 shared.
    let sizes: Vec<usize> = chunks.iter().map(|c| words_of(c.of(&text)).len()).collect();
    assert_eq!(sizes, [400, 400, 280]);
    for pair in chunks.windows(2) {
        let (a, b) = (words_of(pair[0].of(&text)), words_of(pair[1].of(&text)));
        assert_eq!(
            a[a.len() - 40..],
            b[..40],
            "the last 40 words start the next"
        );
    }
    assert_eq!(chunks[0].of(&text).split_whitespace().next(), Some("w0"));
    assert_eq!(chunks[2].of(&text).split_whitespace().last(), Some("w999"));
}

#[test]
fn every_word_is_in_a_chunk_and_no_chunk_cuts_a_word() {
    let text = "Le café à Lisbonne est très bon. ".repeat(300);
    let chunks = chunk(&text, &Estimate);
    assert!(chunks.len() > 1);
    let mut covered = 0;
    for c in &chunks {
        let start = c.offset;
        let end = c.offset + c.len;
        assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
        // Starts and ends on a word's edge.
        assert!(start == 0 || text[..start].ends_with(char::is_whitespace));
        assert!(text[end..].is_empty() || text[end..].starts_with(char::is_whitespace));
        assert!(start <= covered, "no gap before {start}");
        covered = covered.max(end);
        let tokens: usize = c
            .of(&text)
            .split_whitespace()
            .map(|w| Estimate.count(w))
            .sum();
        assert!(tokens <= TARGET, "{tokens} tokens");
    }
    assert_eq!(covered, text.trim_end().len());
}

#[test]
fn the_default_overlap_is_about_forty_tokens() {
    let text = numbered(2000);
    let chunks = chunk(&text, &Estimate);
    for pair in chunks.windows(2) {
        let shared = pair[0].offset + pair[0].len - pair[1].offset;
        let tokens: usize = text[pair[1].offset..pair[1].offset + shared]
            .split_whitespace()
            .map(|w| Estimate.count(w))
            .sum();
        assert!(tokens <= OVERLAP && tokens > OVERLAP / 2, "{tokens}");
    }
}

#[test]
fn a_word_longer_than_a_chunk_is_cut_between_characters() {
    // One run of 5,000 bytes of two-byte characters, between two short words.
    let long: String = core::iter::repeat_n('é', 2500).collect();
    let text = format!("start {long} end");
    let chunks = chunk_with(&text, &Estimate, 100, 10);
    for c in &chunks {
        assert!(text.is_char_boundary(c.offset));
        assert!(text.is_char_boundary(c.offset + c.len));
    }
    assert!(chunks.iter().all(|c| c.len <= MAX_PIECE + "start ".len()));
    let first = chunks.first().unwrap();
    let last = chunks.last().unwrap();
    assert_eq!(first.offset, 0);
    assert_eq!(last.offset + last.len, text.len());
}

#[test]
fn the_same_text_always_gives_the_same_chunks() {
    let text = numbered(1500);
    assert_eq!(chunk(&text, &Estimate), chunk(&text, &Estimate));
}
