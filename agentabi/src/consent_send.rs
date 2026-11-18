//! Whether a line asks for a send (ADR 0002 D3, the P5 close's `Hold` rows):
//! an allow list, not a deny list. A send verb's base form counts only where a
//! request puts one: first in its clause ("send hello to …", "first, send
//! …"), right after "please", "now", "just", "also", "and" or "then", after a
//! wanting "to" ("i want to send …"), or after a polite "can / could / would /
//! will you"; a gerund only first in its clause. A sentence that ends in `?`
//! asks no send unless it opens as a polite request. Anything else asks for no send:
//! the noun ("the blog post about …"), a question, a clause led by "if",
//! "in case", "whether" or "when", and "how to …" or "know what …". A request
//! put some other way is refused, and the person says it again plainly.

use super::{TELL_WORDS, Word, negated};
use alloc::vec::Vec;

/// The base forms of the send rows' cues: a request's verb.
const BASE: &[&str] = &["send", "post", "submit", "resend"];
/// Their gerunds: a send only first in a clause of a sentence that asks
/// nothing ("sending hello to …, now").
const GERUNDS: &[&str] = &["sending", "posting", "submitting", "resending"];

/// Words a request puts right before its verb's base form.
const LEADS: &[&str] = &["please", "now", "just", "also", "and", "then"];

/// Words after which "to" leads a request's verb: "i want to send …". Any
/// other "to" ("used to post", "easy to post") leads none.
const WANTS: &[&str] = &[
    "want", "wants", "wanted", "need", "needs", "like", "going", "have", "has", "ready",
];

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

/// Whether the clause of `at` opens as a polite request: "can you …".
fn polite(words: &[Word], at: usize) -> bool {
    let lead = before(words, at);
    matches!(lead.as_slice(), [first, "you", ..] if POLITE.contains(first))
}

/// Whether the cue at `at` stands where a request puts its verb: a base form
/// first in its clause or after a lead, a polite "can you", or a wanting
/// "to"; a gerund only first in its clause of a sentence that asks nothing.
fn placed(words: &[Word], at: usize) -> bool {
    let text = words[at].text.as_str();
    let base = BASE.contains(&text);
    let gerund = GERUNDS.contains(&text) && !words[at].sentence_asks;
    let lead = before(words, at);
    match lead.as_slice() {
        [] => base || gerund,
        [.., word] if LEADS.contains(word) => base,
        [.., want, "to"] if WANTS.contains(want) => base,
        [.., polite, "you"] if POLITE.contains(polite) => base,
        [.., polite, "you", "please"] if POLITE.contains(polite) => base,
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
    // A sentence that ends in `?` asks, unless it opens as a polite request.
    let asks = words[at].sentence_asks && !polite(words, at);
    words[at].question || led || phrase || told || asks
}

/// Whether the line itself asks for a send.
pub(super) fn asked(words: &[Word]) -> bool {
    words.iter().enumerate().any(|(at, w)| {
        (BASE.contains(&w.text.as_str()) || GERUNDS.contains(&w.text.as_str()))
            && placed(words, at)
            && !telling(words, at)
            && !negated(words, at)
    })
}
