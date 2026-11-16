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

/// Word starts that name going on the network. Un-negated, one asks to go
/// online; negated, one bars every grant.
const NET_STARTS: &[&str] = &[
    "online", "internet", "web", "network", "site", "page", "fetch", "download", "look", "search",
    "brows", "connect", "url", "http",
];

/// Whole words that do the same, too short to match by their start.
const NET_WORDS: &[&str] = &["net", "lookup"];

/// Words that say to stay off the network, wherever they stand.
const OFFLINE_STARTS: &[&str] = &["offline"];

/// Negations a grant also hears, beyond the cue words' list: contractions.
const NEGATING: &[&str] = &[
    "cant", "cannot", "wont", "dont", "didnt", "doesnt", "shouldnt", "mustnt", "neednt", "skip",
    "stop", "nothing",
];

/// A word in a clause that negates, for a grant: the shared list, any `n't`
/// contraction, or [`NEGATING`].
fn negated_here(words: &[Word], at: usize) -> bool {
    negated(words, at)
        || words[..at].iter().any(|w| {
            w.clause == words[at].clause
                && (w.text.ends_with("n't") || NEGATING.contains(&w.text.as_str()))
        })
}

fn net_word(text: &str, row: &Action) -> bool {
    NET_WORDS.contains(&text)
        || NET_STARTS.iter().any(|start| text.starts_with(start))
        || row.cues.iter().any(|cue| cue.split(' ').any(|c| c == text))
}

/// The line says to stay off the network: "offline" anywhere, or a net word
/// or row cue in a clause that negates.
fn offline_said(words: &[Word], row: &Action) -> bool {
    words.iter().enumerate().any(|(at, word)| {
        let text = word.text.as_str();
        OFFLINE_STARTS.iter().any(|start| text.starts_with(start))
            || (net_word(text, row) && negated_here(words, at))
    })
}

/// The line asks to go online: an un-negated net word or row cue, or the word
/// of an app that holds the grant (its package name, a hyphenated one as its
/// words in a row).
fn online_asked(words: &[Word], row: &Action, apps: &[String]) -> bool {
    let counted = |at: usize| !negated_here(words, at);
    let net = words
        .iter()
        .enumerate()
        .any(|(at, w)| net_word(&w.text, row) && counted(at));
    net || apps.iter().any(|app| {
        let parts: Vec<&str> = app.split('-').collect();
        words.windows(parts.len()).enumerate().any(|(at, run)| {
            run.iter().zip(&parts).all(|(w, part)| w.text == *part)
                && (at..at + parts.len()).all(counted)
        })
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
    if !online_asked(words, row, &holders(net, row, call)) {
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
