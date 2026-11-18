//! Whether a line asks for a send (ADR 0002 D3, the P5 close's `Hold` rows):
//! a send verb the line says itself, as a verb, in a clause that asks for no
//! telling and does not say not to. A noun ("the post about hooks.example",
//! "the sender"), a question ("what's the post on it?"), a describe or "how"
//! line ("I wonder whether to send it") asks for none, whatever a view said.

use super::{QUESTION_WORDS, TELL_WORDS, Word, negated};

/// The verb forms of the send rows' cues: exact words, never a prefix.
const SEND_VERBS: &[&str] = &[
    "send",
    "sends",
    "sending",
    "post",
    "posting",
    "submit",
    "submitting",
];

/// Words after which a cue is a noun: determiners, possessives, and the
/// prepositions a topic follows.
const BEFORE_NOUNS: &[&str] = &[
    "the", "a", "an", "my", "your", "our", "their", "his", "her", "its", "this", "that", "these",
    "those", "latest", "last", "next", "first", "new", "old", "about", "for", "of",
];

/// Phrases that make a clause a telling one, before the cue: "I know how to
/// send", "I wonder whether to post".
const TELLING_PHRASES: &[&[&str]] = &[
    &["know", "how"],
    &["wonder", "whether"],
    &["wonder", "if"],
    &["wondering", "whether"],
    &["wondering", "if"],
];

/// Whether the cue at `at` stands in a clause that asks to be told: a question
/// (`what's` and `how's` included), one led by a tell word, a tell word or a
/// telling phrase before the cue.
fn telling(words: &[Word], at: usize) -> bool {
    let clause = words[at].clause;
    let mut before = words[..at].iter().filter(|w| w.clause == clause);
    let first = words
        .iter()
        .find(|w| w.clause == clause)
        .map(|w| w.text.as_str());
    // `what's` and `how's` ask as `what` and `how` do; a polite "can you"
    // is no question (the clause's own flag says so).
    let led = first.is_some_and(|w| {
        w.strip_suffix("'s")
            .is_some_and(|bare| QUESTION_WORDS.contains(&bare))
            || TELL_WORDS.contains(&w)
    });
    let phrase = (0..at).any(|i| {
        words[i].clause == clause
            && TELLING_PHRASES.iter().any(|p| {
                words
                    .get(i..i + p.len())
                    .is_some_and(|run| run.iter().zip(*p).all(|(w, t)| w.text == *t))
            })
    });
    words[at].question || led || phrase || before.any(|w| TELL_WORDS.contains(&w.text.as_str()))
}

/// Whether the line itself asks for a send.
pub(super) fn asked(words: &[Word]) -> bool {
    words.iter().enumerate().any(|(at, w)| {
        SEND_VERBS.contains(&w.text.as_str())
            && !(at > 0
                && (BEFORE_NOUNS.contains(&words[at - 1].text.as_str())
                    || words[at - 1].text.ends_with("'s")))
            && !telling(words, at)
            && !negated(words, at)
    })
}
