//! The line as words: clauses, sentences, tokens and quoted spans, and where a
//! phrase matches them.

use super::*;

/// One word of the line, lower case.
#[derive(Debug)]
pub(super) struct Word {
    pub(super) text: alloc::string::String,
    /// Which clause of the line it is in, counted from 0.
    pub(super) clause: usize,
    /// Whether that clause is a question: it starts with a question word (and is
    /// not a polite request) or ends with `?`.
    pub(super) question: bool,
    /// Which sentence of the line it is in, counted from 0: a sentence ends at
    /// `.` `!` `?` or `;`.
    pub(super) sentence: usize,
    /// Whether that sentence ends with `?`, whatever its clauses.
    pub(super) sentence_asks: bool,
    /// Which token of the line it is in: words joined by one `-`, `.`, `/` or
    /// `_` ("to-delete", "round-robin.md") share one, as a name's words do,
    /// and so do all the words of a quoted span ("to delete" in quotes).
    pub(super) token: usize,
}

/// Marks that open or close a quoted span: straight and curly double quotes.
pub(super) const QUOTES: [char; 3] = ['"', '\u{201C}', '\u{201D}'];

/// The line's words: letters, digits and apostrophes, lower case. Everything else
/// separates, so "round-robin", "round robin" and "Round Robin." read alike.
pub(super) fn words(text: &str) -> Vec<Word> {
    // Curly apostrophes and their look-alikes, as phone keyboards and pastes
    // write them, are the straight one: "Don’t" is "don't".
    let folded: alloc::string::String = text
        .chars()
        .map(|c| match c {
            '\u{2019}' | '\u{2018}' | '\u{2BC}' | '\u{60}' | '\u{B4}' | '\u{2032}' | '\u{FF07}'
            | '\u{2B9}' | '\u{55A}' => '\'',
            c => c,
        })
        .collect();
    let text = folded.as_str();
    let is_word = |c: char| c.is_alphanumeric() || c == '\'';
    let mut clause = 0;
    let mut sentence = 0;
    let mut token = 0;
    let mut quoted = false;
    let mut asked_sentences: Vec<usize> = Vec::new();
    let mut out: Vec<Word> = Vec::new();
    let mut asked = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(|c: char| !is_word(c)).unwrap_or(rest.len());
        let word = rest[..end].trim_matches('\'').to_lowercase();
        rest = &rest[end..];
        let gap = rest.find(is_word).unwrap_or(rest.len());
        let had_word = !word.is_empty();
        if had_word {
            if CLAUSE_WORDS.contains(&word.as_str()) {
                clause += 1;
            }
            out.push(Word {
                text: word,
                clause,
                question: false,
                sentence,
                sentence_asks: false,
                token,
            });
        }
        if rest[..gap].contains('?') {
            asked.push(clause);
        }
        // A dot between two word characters is inside a word, a host's or a
        // number's ("hooks.example", "1.5"): it ends no clause or sentence.
        let inside = had_word && &rest[..gap] == "." && gap < rest.len();
        if !inside && rest[..gap].contains([',', ';', '.', '!', '?', ':']) {
            clause += 1;
        }
        if rest[..gap].contains('?') {
            asked_sentences.push(sentence);
        }
        // A run of dots ("Don't... remove it") trails off: it ends a clause,
        // not a sentence.
        let trails = rest[..gap].contains("..");
        if !inside
            && (rest[..gap].contains([';', '!', '?']) || (rest[..gap].contains('.') && !trails))
        {
            sentence += 1;
        }
        // A quoted span ("to delete") is one token, as a joined name is.
        let quotes = rest[..gap].matches(QUOTES).count();
        let opens = quotes % 2 == 1 && !quoted && rest[gap..].contains(QUOTES);
        let closes = quotes % 2 == 1 && quoted;
        let joined = had_word && gap < rest.len() && matches!(&rest[..gap], "-" | "." | "/" | "_");
        if !(joined || (quoted && !closes)) {
            token += 1;
        }
        quoted = (quoted && !closes) || opens;
        rest = &rest[gap..];
    }
    for c in 0..=clause {
        let mut in_clause = out.iter().filter(|w| w.clause == c);
        let first = in_clause.next().map(|w| w.text.clone());
        let second = in_clause.next().map(|w| w.text.clone());
        let polite = first.as_deref().is_some_and(|w| POLITE.contains(&w))
            && matches!(second.as_deref(), Some("you" | "u"));
        let question = !polite
            && (asked.contains(&c)
                || first
                    .as_deref()
                    .is_some_and(|w| QUESTION_WORDS.contains(&w)));
        for w in out.iter_mut().filter(|w| w.clause == c) {
            w.question = question;
        }
    }
    for w in out.iter_mut() {
        w.sentence_asks = asked_sentences.contains(&w.sentence);
    }
    out
}

/// Where `phrase`'s words appear in a row in `words`. With `prefix`, its last word
/// need only start a word of the line, as a cue does ("polic" in "policy").
pub(super) fn matches(words: &[Word], phrase: &str, prefix: bool) -> Vec<usize> {
    let parts = self::words(phrase);
    let Some((last, first)) = parts.split_last() else {
        return Vec::new();
    };
    if words.len() < parts.len() {
        return Vec::new();
    }
    (0..=words.len() - parts.len())
        .filter(|&at| {
            first
                .iter()
                .zip(&words[at..])
                .all(|(p, w)| p.text == w.text)
                && {
                    let w = &words[at + first.len()].text;
                    if prefix {
                        w.starts_with(last.text.as_str())
                    } else {
                        *w == last.text
                    }
                }
        })
        .collect()
}

pub(super) fn fold_eq(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.text == y.text)
}
