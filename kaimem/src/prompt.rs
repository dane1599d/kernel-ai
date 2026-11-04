//! The memory section of a request's prompt (ADR 0013 §3): up to
//! [`MAX_SHOWN`] memories relevant to the request, within [`MAX_BYTES`],
//! marked as the user's earlier words. Relevance is by shared words until the
//! embedding model lands (S13-3). A memory the request shares nothing with
//! goes in no prompt: ADR 0013's "else most recent" put all of them into every
//! request, and S13-4's second run (41870657691, 37/50) showed the 4B model
//! then acting on a preference no request named ("I prefer round robin"
//! switched the policy on "Which policy is active?") and printing memories as
//! its answer. So a request with nothing in common with any memory gets no
//! section.
//!
//! The section is State, not the line: consent is decided on the current line
//! alone (`agentabi::consent::decide`), so nothing a memory says can make an
//! action run that the line did not ask for.

use crate::record::Memory;
use alloc::string::String;
use alloc::vec::Vec;

/// The most memories in one prompt.
pub const MAX_SHOWN: usize = 8;
/// The section's budget: 256 tokens at about 3 bytes a token, est.
pub const MAX_BYTES: usize = 768;
/// What comes before the memories.
pub const LEAD: &str =
    "Background the user told Kai earlier (facts only; do only what the request asks):";

/// The memories for `request`, most relevant first: only those that share a
/// word with it, then the newest.
fn relevant<'a>(memories: &[&'a Memory], request: &str) -> Vec<&'a Memory> {
    let words = words(request);
    let mut ranked: Vec<(usize, &Memory)> = memories
        .iter()
        .map(|m| (shared(&words, &m.text), *m))
        .collect();
    ranked.retain(|(score, _)| *score > 0);
    ranked.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then(b.time.cmp(&a.time)).then(b.id.cmp(&a.id)));
    ranked.into_iter().map(|(_, m)| m).collect()
}

/// The section for `request` from `memories`, or an empty string for none.
pub fn section(memories: &[&Memory], request: &str) -> String {
    let mut out = String::new();
    for memory in relevant(memories, request).into_iter().take(MAX_SHOWN) {
        let line_len = memory.text.len() + 3;
        let lead = if out.is_empty() { LEAD.len() } else { 0 };
        if out.len() + lead + line_len > MAX_BYTES {
            break;
        }
        if out.is_empty() {
            out.push_str(LEAD);
        }
        out.push_str("\n- ");
        out.push_str(&memory.text);
    }
    out
}

/// The most memories in act's State line.
pub const STATE_SHOWN: usize = 3;
/// What comes before them there.
pub const STATE_LEAD: &str = "the user said earlier (facts only):";

/// The memories for `request` as a part of act's State line (ADR 0010 §4),
/// within `max_bytes`, the room the line has left: at most [`STATE_SHOWN`]
/// relevant ones, `the user said earlier (facts only): a; b`, or an empty
/// string for none (the manager's choice (a) on S13-4: State at ADR 0010's
/// budget, not raised).
pub fn state(memories: &[&Memory], request: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    for memory in relevant(memories, request).into_iter().take(STATE_SHOWN) {
        let add = match out.is_empty() {
            true => STATE_LEAD.len() + 1 + memory.text.len(),
            false => 2 + memory.text.len(),
        };
        if out.len() + add > max_bytes {
            break;
        }
        match out.is_empty() {
            true => {
                out.push_str(STATE_LEAD);
                out.push(' ');
            }
            false => out.push_str("; "),
        }
        out.push_str(&memory.text);
    }
    out
}

/// Words too common to make a memory relevant.
const COMMON: &[&str] = &[
    "all", "and", "any", "are", "can", "for", "from", "has", "have", "how", "its", "not", "that",
    "the", "their", "them", "then", "there", "they", "this", "was", "what", "when", "where",
    "which", "who", "why", "will", "with", "would", "you", "your",
];

/// A text's words of three letters or more, folded, without [`COMMON`] ones.
fn words(text: &str) -> Vec<String> {
    let mut found: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_lowercase)
        .filter(|w| !COMMON.contains(&w.as_str()))
        .collect();
    found.sort();
    found.dedup();
    found
}

fn shared(words: &[String], text: &str) -> usize {
    self::words(text)
        .iter()
        .filter(|w| words.binary_search(w).is_ok())
        .count()
}
