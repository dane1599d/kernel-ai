//! "What can you do with X?" answered from the registry, not by the model. The
//! founder's VirtualBox test (2026-11-14): asked about the CPUs and the GPU,
//! the model offered "increase CPU frequency" and "accelerate graphics", which
//! the OS cannot do. Code lists the rows whose name or description names X,
//! and says so plainly when none does, so the answer never invents an ability.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// How a question about abilities opens, after lowercasing.
const OPENINGS: &[&str] = &[
    "what can you do",
    "what else can you do",
    "what can kai do",
    "can you do anything",
    "is there anything you can do",
    "what are you able to do",
];

/// The words that lead into the topic: "with the cpus", "about the gpu".
const LINKS: &[&str] = &["with", "about", "for", "to", "on"];

/// Words in a topic that name nothing.
const FILLER: &[&str] = &[
    "the", "my", "a", "an", "this", "that", "these", "those", "your",
];

/// The most topic words a question has: a longer line asks something else.
const MAX_TOPIC_WORDS: usize = 3;

/// The topic of an abilities question ("cpus" in "what can you do with the
/// cpus?"), or `None` when `line` is not one, or asks about everything.
fn topic(line: &str) -> Option<Vec<String>> {
    let lower = line
        .trim()
        .trim_end_matches(['?', '.', '!'])
        .to_ascii_lowercase();
    let rest = OPENINGS
        .iter()
        .find_map(|opening| lower.strip_prefix(opening))?;
    let mut words = rest.split_whitespace();
    if !LINKS.contains(&words.next()?) {
        return None;
    }
    let topic: Vec<String> = words
        .filter(|word| !FILLER.contains(word))
        .map(stem)
        .collect();
    (!topic.is_empty() && topic.len() <= MAX_TOPIC_WORDS).then_some(topic)
}

/// `word` without a plural `s`: "cpus" finds "CPU".
fn stem(word: &str) -> String {
    let word = word.trim_matches(|c: char| !c.is_alphanumeric());
    match word.strip_suffix('s') {
        Some(single) if single.len() >= 3 => String::from(single),
        _ => String::from(word),
    }
}

/// Whether `row` names every word of `topic` in its name or description.
fn names(row: &agentabi::Action, topic: &[String]) -> bool {
    let text = format!("{} {}", row.name.replace('_', " "), row.doc).to_ascii_lowercase();
    topic.iter().all(|word| text.contains(word.as_str()))
}

/// The answer to `line` if it asks what Kai can do with something: each row
/// that names it, or that none does. `None` for any other line.
pub(crate) fn answer(line: &str) -> Option<String> {
    let topic = topic(line)?;
    let subject = topic.join(" ");
    let found: Vec<String> = agentabi::ROWS
        .iter()
        .filter(|row| names(row, &topic))
        .map(|row| format!("- {}: {}", row.name, row.doc))
        .collect();
    Some(match found.is_empty() {
        true => format!(
            "None of my actions works with the {subject}, so I can't do anything with it here."
        ),
        false => format!("With the {subject}, I can:\n{}", found.join("\n")),
    })
}
