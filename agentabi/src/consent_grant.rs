//! A read from a host only an app's grant grounds (ADR 0002 D3, founder
//! decision 2026-11-16). The app chose that host, not the user, and the app's
//! own words reach the model (its window title in the State line). So such a
//! fetch is held so narrow that it carries nothing the user did not type:
//!
//! 1. Plain text, written in the plan: control and every argument `Literal`,
//!    and every argument's text known. A `System` or `Content` control is
//!    steered; an unknown or labelled value is no path the line gave.
//! 2. The line asks to go online: an un-negated net word, a cue of the row, or
//!    the word of an app that holds the grant ("the weather in Leeds" for
//!    `weather`). A clause that says not to ("don't go online", "without
//!    looking it up", "stay offline") bars it, and says so.
//! 3. A path of at most [`MAX_WORDS`] line words: `""`, `/`, or `/` and one
//!    contiguous run of the line's words, in order, lower case, each once,
//!    joined by `/`: no query, no other character.
//! 4. One such fetch in a plan, and none in a loop (the shell's plan lists a
//!    looped call twice): any more asks.
//!
//! What still leaks: which host of the app's grants, and which run of up to
//! three of the user's own words, a plan picked.

use super::{Call, Decision, Label, Value, Why, Word, negated, value_of};
use crate::{Action, ArgType, Tier};
use alloc::string::String;
use alloc::vec::Vec;
use nethosts::Grounding;

/// The most line words a grant-only path holds.
pub const MAX_WORDS: usize = 3;

/// The source a `System` control names: the OS's own state decided the call.
pub const SYSTEM: &str = "system";

/// Word starts that plainly mean the internet: one, un-negated, asks to go
/// online.
const ONLINE_STARTS: &[&str] = &[
    "online", "internet", "web", "brows", "download", "url", "http",
];

/// Whole words that do the same.
const ONLINE_WORDS: &[&str] = &["fetch", "fetched", "fetching", "dns", "ping", "google"];

/// Phrases that do the same, matched whole: "look" alone is any look, and
/// "up" alone is any "back it up".
const ONLINE_PHRASES: &[&[&str]] = &[
    &["look", "up"],
    &["look", "it", "up"],
    &["look", "that", "up"],
    &["looking", "up"],
    &["looking", "it", "up"],
];

/// Word starts that touch the network at all. With a word that negates
/// anywhere in the line, one bars every grant: when unsure, the line said to
/// stay offline.
const NET_HINTS: &[&str] = &[
    "online", "internet", "web", "brows", "download", "url", "http", "net", "connect", "search",
    "look", "site", "page", "fetch", "wifi", "lookup", "dns", "ping", "google",
];

/// Negations a grant also hears, beyond the cue words' lists and any `n't`
/// contraction.
const NEGATING: &[&str] = &[
    "cant", "cannot", "wont", "dont", "didnt", "doesnt", "shouldnt", "mustnt", "neednt", "skip",
    "stop", "nothing", "nor", "neither", "instead",
];

/// A word that negates, for a grant: the shared lists, any `n't`
/// contraction, or [`NEGATING`].
fn negating(text: &str) -> bool {
    super::NEGATIONS.contains(&text)
        || super::OFF_WORDS.contains(&text)
        || NEGATING.contains(&text)
        || text.ends_with("n't")
}

/// A word in a clause that negates, for a grant.
fn negated_here(words: &[Word], at: usize) -> bool {
    negated(words, at)
        || words[..at]
            .iter()
            .any(|w| w.clause == words[at].clause && negating(&w.text))
}

/// Whether `phrase` stands at `at` in `words`.
fn phrase_at(words: &[Word], at: usize, phrase: &[&str]) -> bool {
    words
        .get(at..at + phrase.len())
        .is_some_and(|run| run.iter().zip(phrase).all(|(w, p)| w.text == *p))
}

/// The line says to stay off the network: "offline" or "off line" anywhere,
/// or a word that touches the network (or a word of the row's cues) in a line
/// that holds any word that negates, before or after it.
fn offline_said(words: &[Word], row: &Action) -> bool {
    let off = words.iter().enumerate().any(|(at, w)| {
        w.text.starts_with("offline") || (w.text == "off" && phrase_at(words, at + 1, &["line"]))
    });
    let hint = words.iter().any(|w| {
        NET_HINTS.iter().any(|start| w.text.starts_with(start))
            || row
                .cues
                .iter()
                .any(|cue| cue.split(' ').any(|c| c == w.text))
    });
    off || (hint && words.iter().any(|w| negating(&w.text)))
}

/// The line asks to go online: a word or phrase that plainly means the
/// internet, un-negated, or the word of an app that holds the grant when that
/// word is no common English word ("weather" for `weather`, never `my`).
fn online_asked(words: &[Word], apps: &[String]) -> bool {
    let counted = |at: usize| !negated_here(words, at);
    let net = words.iter().enumerate().any(|(at, w)| {
        let text = w.text.as_str();
        let word = ONLINE_WORDS.contains(&text)
            || ONLINE_STARTS.iter().any(|start| text.starts_with(start));
        let phrase = ONLINE_PHRASES.iter().any(|p| phrase_at(words, at, p));
        (word || phrase) && counted(at)
    });
    net || apps.iter().filter(|app| app_word(app)).any(|app| {
        let parts: Vec<&str> = app.split('-').collect();
        words.windows(parts.len()).enumerate().any(|(at, run)| {
            run.iter().zip(&parts).all(|(w, part)| w.text == *part)
                && (at..at + parts.len()).all(counted)
        })
    })
}

/// Whether an app's package name may stand for its site in a line: each part
/// at least three letters and none a common English, kind or leading word.
fn app_word(app: &str) -> bool {
    app.split('-').all(|part| {
        part.len() >= 3
            && super::ENGLISH.binary_search(&part).is_err()
            && !super::KIND_WORDS.contains(&part)
            && !super::DETERMINERS.contains(&part)
    })
}

/// A path the line gives: `""`, `/`, or `/` and one contiguous run of up to
/// [`MAX_WORDS`] of its words, in order, each once, lower case, joined by `/`.
fn path_ok(words: &[Word], text: &str) -> bool {
    if text.is_empty() || text == "/" {
        return true;
    }
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    if !rest
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'/')
    {
        return false;
    }
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() > MAX_WORDS || parts.iter().any(|p| p.is_empty()) {
        return false;
    }
    let mut seen: Vec<&str> = Vec::new();
    for part in &parts {
        if seen.contains(part) {
            return false;
        }
        seen.push(part);
    }
    words.windows(parts.len()).enumerate().any(|(at, run)| {
        run.iter().zip(&parts).all(|(w, part)| w.text == *part)
            && (at..at + parts.len()).all(|i| !negated_here(words, i))
    })
}

/// Every non-host argument is known, plan-written text the line gives.
fn arguments_ok(words: &[Word], row: &Action, call: &Call) -> bool {
    row.args
        .iter()
        .filter(|arg| !matches!(arg.ty, ArgType::Host))
        .filter_map(|arg| value_of(call, arg.name))
        .all(|value: Value| {
            // `path_ok` refuses `UNKNOWN` too; it is said here outright so an
            // unknown value never rests on the path check's character set.
            value.label == Label::Literal
                && value.text != super::UNKNOWN
                && path_ok(words, value.text)
        })
}

/// The apps that hold a grant for a host of `call`.
fn holders(net: &Grounding, row: &Action, call: &Call) -> Vec<String> {
    row.args
        .iter()
        .filter(|arg| matches!(arg.ty, ArgType::Host))
        .filter_map(|arg| value_of(call, arg.name))
        .filter_map(|value| nethosts::Host::parse(value.text).ok())
        .flat_map(|host| net.granted.holders(&host))
        .collect()
}

/// The decision for a read whose host only a grant grounds.
pub(super) fn read(words: &[Word], net: &Grounding, row: &Action, call: &Call) -> Decision {
    match call.control {
        Label::Content(source) => return Decision::new(Tier::Ask, Why::GrantSteered(source)),
        Label::System => return Decision::new(Tier::Ask, Why::GrantSteered(SYSTEM)),
        Label::Literal => {}
    }
    if offline_said(words, row) {
        return Decision::new(Tier::Ask, Why::GrantOffline);
    }
    if !online_asked(words, &holders(net, row, call)) {
        return Decision::new(Tier::Ask, Why::Ungrounded);
    }
    if !arguments_ok(words, row, call) {
        return Decision::new(Tier::Ask, Why::GrantPath);
    }
    Decision::new(row.min_tier.max(Tier::Run), Why::GrantRead)
}

/// One grant-only read in a plan: with more, each asks.
pub(super) fn at_most_one(decisions: Vec<Decision>) -> Vec<Decision> {
    let reads = decisions.iter().filter(|d| d.why == Why::GrantRead).count();
    decisions
        .into_iter()
        .map(|d| match d.why {
            Why::GrantRead if reads > 1 => Decision::new(Tier::Ask, Why::GrantMany),
            _ => d,
        })
        .collect()
}
