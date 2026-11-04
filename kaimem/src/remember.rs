//! Who may make a memory (ADR 0013 §3, question 5's default: both).
//!
//! - The user's own line that asks, "remember that I use metric units", runs:
//!   the memory is the line's own words ([`from_line`]), not the model's.
//! - A memory the model proposes asks, with its exact text: `Remember "you
//!   prefer metric units"? y/n`. A setting turns proposals off.
//! - A memory whose text came from Content (a file, a note, a search result, a
//!   page) is refused, whoever asks: read data can never plant one.
//! - An agent writes only to its own scope, and only with `memory write`.

use crate::record::{MAX_TEXT, Scope, text_problem};
use agentabi::consent::Label;
use alloc::format;
use alloc::string::{String, ToString};

/// What wants a memory made.
#[derive(Clone, Copy, Debug)]
pub enum Asker<'a> {
    /// The user's line, as typed.
    Line(&'a str),
    /// The model proposed `text` for the user, labelled as the plan labels it.
    Proposal { text: &'a str, label: Label },
    /// An agent's own memory: `may_write` is its manifest's `memory write`.
    Agent {
        name: &'a str,
        may_write: bool,
        text: &'a str,
        label: Label,
    },
}

/// What happens to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Kept now, with this text and scope.
    Runs { text: String, scope: Scope },
    /// Kept only on the user's yes to `question`, which shows the exact text.
    Asks {
        text: String,
        scope: Scope,
        question: String,
    },
    /// Not kept, and why.
    Refused(String),
}

/// Decides `asker`'s memory; `proposals` is the setting that lets the model
/// propose them.
pub fn decide(asker: Asker, proposals: bool) -> Verdict {
    match asker {
        Asker::Line(line) => match from_line(line) {
            Some(text) => checked(text, Scope::All, false),
            None => Verdict::Refused(String::from("the line does not ask Kai to remember")),
        },
        Asker::Proposal { label, .. } if content(label).is_some() => from_content(label),
        Asker::Proposal { .. } if !proposals => {
            Verdict::Refused(String::from("memory proposals are off in Settings"))
        }
        Asker::Proposal { text, .. } => checked(text, Scope::All, true),
        Asker::Agent { name, .. } if !crate::record::is_agent_name(name) => {
            Verdict::Refused(format!("{name:?} is not an agent's name"))
        }
        Asker::Agent {
            may_write: false,
            name,
            ..
        } => Verdict::Refused(format!("{name} may not write memories")),
        Asker::Agent { label, .. } if content(label).is_some() => from_content(label),
        Asker::Agent { name, text, .. } => checked(text, Scope::Agent(name.to_string()), false),
    }
}

fn content(label: Label) -> Option<&'static str> {
    match label {
        Label::Content(source) => Some(source),
        _ => None,
    }
}

fn from_content(label: Label) -> Verdict {
    let source = content(label).unwrap_or("read data");
    Verdict::Refused(format!(
        "a memory is never made from content ({source}); say \"remember …\" yourself"
    ))
}

fn checked(text: &str, scope: Scope, asks: bool) -> Verdict {
    let text = text.trim();
    if let Some(why) = text_problem(text) {
        return Verdict::Refused(String::from(why));
    }
    match asks {
        true => Verdict::Asks {
            text: text.to_string(),
            question: question(text),
            scope,
        },
        false => Verdict::Runs {
            text: text.to_string(),
            scope,
        },
    }
}

/// The proposal's question: the exact text, quoted.
pub fn question(text: &str) -> String {
    format!("Remember \"{text}\"? y/n")
}

/// Words that may come before "remember" in a line that asks.
const LEADS: &[&str] = &[
    "please", "kai", "hey", "ok", "okay", "can", "could", "would", "will", "you",
];

/// The memory a line asks for: what follows "remember" (and "that"), in the
/// user's own words. `None` for a line that does not ask, such as "do you
/// remember my units?", "don't remember that" or "I remember it".
pub fn from_line(line: &str) -> Option<&str> {
    let mut rest = line.trim();
    let polite = loop {
        let (word, after) = rest.split_once(' ')?;
        let bare = word.trim_end_matches([',', ':']).to_ascii_lowercase();
        match bare.as_str() {
            "remember" => break rest.len() != line.trim().len(),
            lead if LEADS.contains(&lead) => rest = after.trim_start(),
            _ => return None,
        }
    };
    // Only "can/could/would/will you remember …?" may end in a question mark;
    // "remember my units?" and "do you remember …?" ask, they do not tell.
    let led = line.trim()[..line.trim().len() - rest.len()].to_ascii_lowercase();
    let request = polite
        && led.split([' ', ',']).any(|w| w == "you")
        && led
            .split([' ', ','])
            .any(|w| ["can", "could", "would", "will"].contains(&w));
    let mut text = rest["remember".len()..].trim_start();
    if text.ends_with('?') && !request {
        return None;
    }
    for lead in ["that ", "this: ", "this "] {
        if let Some(after) = strip_prefix_fold(text, lead) {
            text = after;
            break;
        }
    }
    let text = text.trim_end_matches(['.', '?', '!']).trim();
    let bare = ["that", "this", "it"]
        .iter()
        .any(|w| text.eq_ignore_ascii_case(w));
    (!text.is_empty() && !bare && text.len() <= MAX_TEXT).then_some(text)
}

fn strip_prefix_fold<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}
