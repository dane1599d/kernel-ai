//! What kind of line it is: a want or complaint (the vague path), a question,
//! a request to be told, and whether a value names a thing.

use super::*;

/// Words that state a want or a complaint about how the system feels: the only
/// lines the vague path reads as asking for a change.
pub(super) const WANT_WORDS: &[&str] = &[
    "snappier",
    "snappy",
    "faster",
    "quicker",
    "smoother",
    "slow",
    "slower",
    "sluggish",
    "laggy",
    "lag",
    "lagging",
    "lags",
    "choppy",
    "janky",
    "stutters",
    "stuttering",
    "unresponsive",
];
/// Phrases that do the same.
pub(super) const WANT_PHRASES: &[&str] = &[
    "speed it up",
    "speed things up",
    "put it back",
    "put that back",
    "change it back",
    "set it back",
    "go back",
    "the way it was",
    "do it again",
    "do that again",
    "same again",
    "once more",
];

/// Lines that ask for the last thing again (P1 plan section 4): "do" starts
/// them as an order, not a question.
pub(super) const REPEATS: &[&str] = &["do it again", "do that again"];

/// Whether the line states a want or a complaint ([`WANT_WORDS`], [`WANT_PHRASES`]).
pub(super) fn wants(words: &[Word]) -> bool {
    words.iter().any(|w| WANT_WORDS.contains(&w.text.as_str()))
        || WANT_PHRASES
            .iter()
            .any(|phrase| !matches(words, phrase, false).is_empty())
}

/// Words that start a question in English.
pub(super) const QUESTION_WORDS: &[&str] = &[
    "how", "what", "which", "who", "whom", "whose", "when", "where", "why", "is", "are", "was",
    "were", "does", "do", "did", "can", "could", "will", "would", "should", "has", "have",
];

/// Whether `line` is a question: it ends with `?`, or starts with a question word
/// ("how many CPUs are there", "is the network up").
pub(super) fn is_question(line: &str) -> bool {
    let line = line.trim();
    let repeat = REPEATS.iter().any(|r| {
        line.get(..r.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(r))
    });
    line.ends_with('?')
        || !repeat
            && words(line)
                .first()
                .is_some_and(|w| QUESTION_WORDS.contains(&w.text.as_str()))
}

/// Words that start a line asking to be told something, not for a change.
pub(super) const TELL_WORDS: &[&str] = &[
    "describe",
    "explain",
    "tell",
    "show",
    "list",
    "summarize",
    "summarise",
    "report",
];

/// Question words that only ever ask to be told: not `do`, `can` or `will`,
/// which also lead orders ("do the same for the editor").
pub(super) const WH_WORDS: &[&str] = &[
    "how", "what", "which", "who", "whom", "whose", "when", "where", "why",
];

/// Whether `line` only asks to be told something: one ending in `?`, one led by
/// a wh-word ("how many cpus"), or one led by a telling word ("describe this",
/// "explain the policy"). A change consent [`Why::Added`] to such a line
/// answers nothing it asked, whatever the State or a page says (#721).
pub fn asks_to_be_told(line: &str) -> bool {
    let words = words(line);
    if wants(&words) {
        return false;
    }
    let first = words.first().map(|w| w.text.as_str());
    line.trim().ends_with('?')
        || first.is_some_and(|w| WH_WORDS.contains(&w) || TELL_WORDS.contains(&w))
}

/// Whether `text` names a thing: some word of it is neither a kind word nor a
/// determiner. "notes" and "the llm service" do; "the service" and "it" do not.
pub(super) fn names_a_thing(text: &str) -> bool {
    words(text).iter().any(|w| {
        let w = w.text.as_str();
        !KIND_WORDS.contains(&w) && !DETERMINERS.contains(&w)
    })
}
