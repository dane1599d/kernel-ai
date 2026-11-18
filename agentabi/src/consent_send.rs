//! Whether a line asks for a send (ADR 0002 D3, the P5 close's `Hold` rows):
//! an allow list, not a deny list. A send verb counts only where a request
//! puts one: first in its clause ("send hello to …", "first, send …"), or
//! right after "please", "now", "just", "also", "and", "then", "to", or a
//! polite "can / could / would / will you". Anything else asks for no send:
//! the noun ("the blog post about …"), a question, a clause led by "if",
//! "in case", "whether" or "when", and "how to …" or "know what …". A request
//! put some other way is refused, and the person says it again plainly.

use super::{TELL_WORDS, Word, negated};
use alloc::vec::Vec;

/// The verb forms of the send rows' cues: exact words, never a prefix.
const SEND_VERBS: &[&str] = &[
    "send",
    "sends",
    "sending",
    "resend",
    "post",
    "posting",
    "submit",
    "submitting",
];

/// Words a request puts right before its verb.
const LEADS: &[&str] = &["please", "now", "just", "also", "and", "then", "to"];

/// The polite requests: "can you send …".
const POLITE: &[&str] = &["can", "could", "would", "will"];

/// Words that lead a clause that only supposes or asks: "if i send …".
const SUPPOSING: &[&str] = &["if", "whether", "when", "unless"];

/// Phrases that make a clause a telling one, before the cue.
const TELLING_PHRASES: &[&[&str]] = &[
    &["how", "to"],
    &["know", "how"],
    &["know", "what"],
    &["know", "if"],
    &["know", "whether"],
    &["wonder", "whether"],
    &["wonder", "if"],
    &["wondering", "whether"],
    &["wondering", "if"],
    &["in", "case"],
];

/// The words of `at`'s clause before it.
fn before(words: &[Word], at: usize) -> Vec<&str> {
    let clause = words[at].clause;
    words[..at]
        .iter()
        .filter(|w| w.clause == clause)
        .map(|w| w.text.as_str())
        .collect()
}

/// Whether the cue at `at` stands where a request puts its verb.
fn placed(words: &[Word], at: usize) -> bool {
    let lead = before(words, at);
    match lead.as_slice() {
        [] => true,
        [.., word] if LEADS.contains(word) => true,
        [.., polite, "you"] if POLITE.contains(polite) => true,
        [.., polite, "you", "please"] if POLITE.contains(polite) => true,
        _ => false,
    }
}

/// Whether the cue at `at` stands in a clause that supposes, asks or tells:
/// a question (not a polite request), a clause led by a supposing word or a
/// tell word, a telling phrase before the cue, or "whats" / "hows".
fn telling(words: &[Word], at: usize) -> bool {
    let lead = before(words, at);
    let first = lead.first().copied().unwrap_or("");
    // "what's", "whats", "how's" and "hows" lead as "what" and "how" do.
    let bare = first
        .strip_suffix("'s")
        .or_else(|| first.strip_suffix('s'))
        .unwrap_or(first);
    let led =
        SUPPOSING.contains(&first) || TELL_WORDS.contains(&first) || matches!(bare, "what" | "how");
    let phrase = lead.windows(2).any(|pair| {
        TELLING_PHRASES
            .iter()
            .any(|p| p.len() == 2 && pair[0] == p[0] && pair[1] == p[1])
    });
    let told = lead.iter().any(|w| TELL_WORDS.contains(w));
    words[at].question || led || phrase || told
}

/// Whether the line itself asks for a send.
pub(super) fn asked(words: &[Word]) -> bool {
    words.iter().enumerate().any(|(at, w)| {
        SEND_VERBS.contains(&w.text.as_str())
            && placed(words, at)
            && !telling(words, at)
            && !negated(words, at)
    })
}
