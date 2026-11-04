//! A chunk's terms for the keyword score: its runs of letters and digits,
//! lower-cased, each with how often it occurs. A run of one character is not a
//! term, and a long one is cut to [`MAX_TERM`] bytes between characters, so the
//! same rule reads a query and a chunk alike.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// The longest term kept, in bytes.
pub const MAX_TERM: usize = 32;
/// Most distinct terms kept for one chunk: the most frequent, ties by term.
pub const MAX_TERMS: usize = 1024;

/// The terms of `text` in order: each run of letters and digits, lower-cased
/// and cut to [`MAX_TERM`] bytes; runs of one character are left out.
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut chars = 0;
    let mut flush = |word: &mut String, chars: &mut usize| {
        if *chars > 1 {
            out.push(core::mem::take(word));
        } else {
            word.clear();
        }
        *chars = 0;
    };
    for c in text.chars() {
        if !c.is_alphanumeric() {
            flush(&mut word, &mut chars);
            continue;
        }
        chars += 1;
        for lower in c.to_lowercase() {
            if word.len() + lower.len_utf8() <= MAX_TERM {
                word.push(lower);
            }
        }
    }
    flush(&mut word, &mut chars);
    out
}

/// The distinct terms of `text` with their counts, sorted by term, at most
/// [`MAX_TERMS`] of them.
pub fn counted(text: &str) -> Vec<(String, u16)> {
    let mut counts: BTreeMap<String, u16> = BTreeMap::new();
    for word in words(text) {
        let n = counts.entry(word).or_insert(0);
        *n = n.saturating_add(1);
    }
    let mut terms: Vec<(String, u16)> = counts.into_iter().collect();
    if terms.len() > MAX_TERMS {
        terms.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        terms.truncate(MAX_TERMS);
        terms.sort_by(|a, b| a.0.cmp(&b.0));
    }
    terms
}
