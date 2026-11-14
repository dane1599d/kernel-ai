//! Whether a line typed after a clarifying question answers it. An answer
//! joins the request it clarifies ("switch the policy" + "round robin"); a
//! line that reads as a request of its own ("show me the cpus") runs alone,
//! and the question is dropped. Code decides, from the line's first word, so
//! the model's input is unchanged.

/// Words a request or a question opens with. A clarifying answer names a
/// thing ("sshd", "the second one", "round robin"); it rarely opens with one.
const LEADS: &[&str] = &[
    "show",
    "list",
    "open",
    "close",
    "start",
    "stop",
    "restart",
    "run",
    "kill",
    "install",
    "uninstall",
    "remove",
    "delete",
    "add",
    "set",
    "switch",
    "change",
    "make",
    "create",
    "build",
    "find",
    "search",
    "fetch",
    "look",
    "tell",
    "give",
    "help",
    "undo",
    "what",
    "what's",
    "whats",
    "how",
    "why",
    "when",
    "where",
    "who",
    "is",
    "are",
    "can",
    "could",
    "would",
    "will",
    "do",
    "does",
    "please",
];

/// The fewest words a line needs to read as a request of its own: a single
/// word ("stop", offered as an option) is always an answer.
const REQUEST_WORDS: usize = 2;

/// Whether `line`, typed while a question is open, answers it rather than
/// starting a new request.
pub(crate) fn answers(line: &str) -> bool {
    let words: alloc::vec::Vec<&str> = line.split_whitespace().collect();
    let Some(first) = words.first() else {
        return true;
    };
    let first = first
        .trim_end_matches([',', '.', '?', '!', ':'])
        .to_ascii_lowercase();
    words.len() < REQUEST_WORDS || !LEADS.contains(&first.as_str())
}
