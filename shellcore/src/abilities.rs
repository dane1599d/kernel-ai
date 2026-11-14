//! What Kai can do, from the registry rather than the model. The founder's
//! VirtualBox test (2026-11-14): asked about the CPUs and the GPU, the model
//! answered with a `clarify` offering "increase CPU frequency" and "accelerate
//! graphics with GPU", which the OS cannot do.
//!
//! - A short question about what Kai can do with a named thing is answered in
//!   code when rows name it: each row and script call whose name or
//!   description holds the thing as a whole word. A topic no row names, or
//!   one that means everything ("for me", "on this computer"), goes to the
//!   model as before: no match is not proof of no ability.
//! - A model's `clarify` keeps only the options some row backs ([`backed`]).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// How a question about abilities opens, after lowercasing. It may come
/// anywhere in the line: "can you ican you do anything with the gpu?".
const OPENINGS: &[&str] = &[
    "what can you do",
    "what else can you do",
    "what can kai do",
    "can you do anything",
    "could you do anything",
    "is there anything you can do",
    "what are you able to do",
];

/// Openings that name the topic straight after: "can you use the gpu".
const USE_OPENINGS: &[&str] = &["can you use", "could you use", "can you work with"];

/// The words that lead into the topic: "with the cpus", "about the gpu".
const LINKS: &[&str] = &["with", "about", "for", "to", "on"];

/// Words in a topic that name nothing.
const FILLER: &[&str] = &[
    "the", "my", "a", "an", "this", "that", "these", "those", "your", "all", "any", "of",
];

/// Topics that mean everything Kai does: answered by the model, not a list.
const COURTESY: &[&str] = &[
    "me",
    "us",
    "it",
    "please",
    "computer",
    "pc",
    "here",
    "machine",
    "system",
    "os",
    "kai",
    "now",
    "today",
    "things",
    "stuff",
    "everything",
    "anything",
];

/// Words in a clarify option that say what to do, not what with: an option
/// is judged by its other words ("increase CPU frequency" by cpu, frequency).
const VERBS: &[&str] = &[
    "use",
    "adjust",
    "change",
    "set",
    "manage",
    "show",
    "see",
    "check",
    "monitor",
    "increase",
    "decrease",
    "make",
    "do",
    "get",
    "run",
    "switch",
    "view",
    "list",
    "open",
    "control",
    "tune",
    "improve",
    "boost",
    "enable",
    "disable",
    "accelerate",
    "with",
    "for",
    "to",
    "of",
    "and",
    "or",
    "the",
    "a",
    "an",
    "by",
    "in",
    "on",
    "my",
    "your",
    "it",
];

/// The most words a question about abilities has.
const MAX_LINE_WORDS: usize = 12;

/// The most topic words a question has: a longer topic asks something else.
const MAX_TOPIC_WORDS: usize = 3;

/// The shortest word a suffix is stripped from.
const MIN_STEM: usize = 3;

/// `word` lowercased, without its plural or "-ing"/"-er" ending: "cpus" and
/// "CPU" meet, and so do "scheduling" and "scheduler".
fn stem(word: &str) -> String {
    let word = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    for suffix in ["ing", "er", "es", "s"] {
        if let Some(root) = word.strip_suffix(suffix)
            && root.len() >= MIN_STEM
        {
            return String::from(root);
        }
    }
    word
}

/// The stems of `text`'s words, `_` splitting a row name into words.
fn stems(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(stem)
        .collect()
}

/// Every row and script call, each once.
fn registry() -> impl Iterator<Item = &'static agentabi::Action> {
    let rows = agentabi::ROWS.iter();
    let calls = agentabi::SCRIPT_CALLS
        .iter()
        .filter(|call| !agentabi::ROWS.iter().any(|row| row.name == call.name));
    rows.chain(calls)
}

/// Whether `row`'s name or description holds every word of `topic`.
fn names(row: &agentabi::Action, topic: &[String]) -> bool {
    let words = stems(&format!("{} {}", row.name, row.doc));
    topic.iter().all(|word| words.contains(word))
}

/// The topic after an opening, if `line` asks what Kai can do with something.
fn topic(line: &str) -> Option<Vec<String>> {
    if line.split_whitespace().count() > MAX_LINE_WORDS {
        return None;
    }
    let lower = line
        .trim()
        .trim_end_matches(['?', '.', '!'])
        .to_ascii_lowercase();
    let after = |openings: &[&str]| {
        openings
            .iter()
            .filter_map(|opening| lower.rfind(opening).map(|at| at + opening.len()))
            .max()
            .map(|end| String::from(&lower[end..]))
    };
    let rest = match after(USE_OPENINGS) {
        Some(rest) => rest,
        None => {
            let rest = after(OPENINGS)?;
            let mut words = rest.split_whitespace();
            if !LINKS.contains(&words.next()?) {
                return None;
            }
            words.collect::<Vec<_>>().join(" ")
        }
    };
    let topic: Vec<&str> = rest
        .split_whitespace()
        .filter(|word| !FILLER.contains(word))
        .collect();
    if topic.is_empty()
        || topic.len() > MAX_TOPIC_WORDS
        || topic.iter().any(|word| COURTESY.contains(word))
    {
        return None;
    }
    Some(topic.iter().map(|word| stem(word)).collect())
}

/// The answer to `line` if it asks what Kai can do with something that rows
/// name: each of them. `None` for any other line, and for a topic no row
/// names, which the model answers.
pub(crate) fn answer(line: &str) -> Option<String> {
    let topic = topic(line)?;
    let found: Vec<String> = registry()
        .filter(|row| names(row, &topic))
        .map(|row| format!("- {}: {}", row.name, row.doc))
        .collect();
    if found.is_empty() {
        return None;
    }
    Some(format!(
        "With the {}, I can:\n{}",
        topic.join(" "),
        found.join("\n")
    ))
}

/// Whether some row backs a clarify `option`: every word of it that names a
/// thing ("cpu", "frequency" in "increase CPU frequency") is in one row's
/// name or description. An option of verbs alone is kept.
pub(crate) fn backed(option: &str) -> bool {
    // Split as row text is, so "round-robin" meets set_policy's "round-robin".
    let things: Vec<String> = option
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty() && !VERBS.contains(&word.to_ascii_lowercase().as_str()))
        .map(stem)
        .collect();
    things.is_empty() || registry().any(|row| names(row, &things))
}
