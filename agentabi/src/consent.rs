//! Consent from the user's line (ADR 0002 D2, D3 and D6), as code with no model:
//! for each call in a plan, whether it runs at once, runs as "Taken as", asks, or is
//! refused. Spike S01-2's tier corpus (`tests/tier_corpus.rs`) measures it; task 7
//! builds `agentcore::execute` on it.
//!
//! The rule: what the line plainly asks for, and can be undone, runs. A change is
//! asked for when one of its row's cues starts a word of the line, not within three
//! words after a negation, and every `from_request` argument is literal in the line,
//! a declared alias, or `focused`/`selected`/`here` for "this", "it", "here" or
//! "selected". A literal counts only if it names a thing: "the service", "that
//! program" or "it" is a kind word, not a name, so `service_stop("service")` from
//! "stop the service" was not asked for. Anything else is added and asks, except the vague path: a plan of one
//! undoable local change, from a line that names no action at all, runs and says
//! what it was taken as. Values read from content (`Label::Content`) cannot steer
//! an undo-less or outbound call, and a line that is not English gets no consent
//! for changes.

use crate::{Action, ArgType, Inverse, Role, Tier, derive, find};
use alloc::vec::Vec;

/// Where a value came from (ADR 0002 D6). Ordered: the least trusted is greatest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Written in the plan before anything was read.
    Literal,
    /// The OS's own state.
    System,
    /// Content read from a source: a note, a page, a file.
    Content(&'static str),
}

/// One argument value and where it came from.
#[derive(Clone, Copy, Debug)]
pub struct Value<'a> {
    pub text: &'a str,
    pub label: Label,
}

/// One call the plan makes: the action, its arguments by name, and what decided
/// that it runs at all (`Content` when it sits under an `if` on read data).
#[derive(Clone, Copy, Debug)]
pub struct Call<'a> {
    pub name: &'a str,
    pub args: &'a [(&'a str, Value<'a>)],
    pub control: Label,
}

/// Why a call got its tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A read on this machine.
    Read,
    /// A fetch from a host the line names or the host table grounds.
    Grounded,
    /// The line asked for it, and it can be undone.
    Asked,
    /// The line asked for it, and it cannot be undone: it asks once.
    NoUndo,
    /// The one change a vague line was read as.
    TakenAs,
    /// The model added it: no cue, a negated cue, or an argument not in the line.
    Added,
    /// A fetch from a host nobody named.
    Ungrounded,
    /// Read content picks its target or payload, or decides that it runs, and it has
    /// no undo or sends something out.
    Steered(&'static str),
    /// Content would go out of the machine.
    Outbound(&'static str),
    /// The line is not English, so it gives no consent for changes in v0.1.
    NotEnglish,
    /// A value no call may take, or a row that never runs.
    NeverValue,
    /// No row has this name.
    Unknown,
}

/// What happens to one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub tier: Tier,
    pub why: Why,
}

impl Decision {
    const fn new(tier: Tier, why: Why) -> Self {
        Self { tier, why }
    }
}

/// Request words that name a site, and the host each one means. A host is grounded
/// only when the line types it or names it here; a label match is not enough.
pub const HOSTS: &[(&str, &str)] = &[
    ("youtube", "www.youtube.com"),
    ("wikipedia", "en.wikipedia.org"),
];

/// Words that turn the next three words' cues off.
const NEGATIONS: &[&str] = &["dont", "don't", "not", "never", "no", "without"];
/// How many words after a negation it reaches.
const NEGATION_REACH: usize = 3;
/// Words that point at what the OS has focused or selected.
const POINTERS: &[&str] = &["this", "it", "here", "selected"];
const SYMBOLS: &[&str] = &["focused", "selected", "here"];
/// Words for a kind of thing, or for one in passing, that name none: "stop the
/// service" names no service. Plurals are listed, not derived, so the app `notes`
/// stays a name.
const KIND_WORDS: &[&str] = &[
    "service",
    "services",
    "program",
    "programs",
    "app",
    "apps",
    "application",
    "process",
    "processes",
    "package",
    "packages",
    "device",
    "component",
    "file",
    "files",
    "folder",
    "server",
    "host",
    "site",
    "website",
    "page",
    "model",
    "rule",
    "note",
    "thing",
    "things",
    "it",
    "that",
    "this",
    "one",
    "them",
    "those",
    "these",
];
/// Words that only lead into a name.
const DETERMINERS: &[&str] = &[
    "the", "a", "an", "my", "your", "our", "its", "some", "any", "that", "this",
];

/// Decides every call of `plan`, made for the user's `line`, against `rows`.
pub fn decide(line: &str, plan: &[Call], rows: &[Action]) -> Vec<Decision> {
    let words = words(line);
    let english = is_english(line);
    let named_any = rows.iter().any(|row| cued(&words, row).is_some());
    let decisions: Vec<Decision> = plan
        .iter()
        .map(|call| decide_one(&words, english, call, rows))
        .collect();
    let changes = plan
        .iter()
        .filter(|call| find(rows, call.name).is_some_and(|row| !row.facts.read_only))
        .count();
    // The vague path: one call in all, an undoable local change with nothing read in
    // it, from an English line that names no action but states a want or a
    // complaint, and asks no question. Default deny: any other line, such as
    // "show me the cpu count" or "how many cpus are there", asked to be told, not
    // for a change, so a change answering it is added and asks.
    if let (
        [call],
        [
            Decision {
                why: Why::Added, ..
            },
        ],
    ) = (plan, decisions.as_slice())
        && changes == 1
        && english
        && !named_any
        && wants(&words)
        && !is_question(line)
        && let Some(row) = find(rows, call.name)
        && derive(row) == Tier::Run
        && !row.facts.open_world
        && !steered_at_all(call)
    {
        return alloc::vec![Decision::new(Tier::Run, Why::TakenAs)];
    }
    decisions
}

fn decide_one(words: &[Word], english: bool, call: &Call, rows: &[Action]) -> Decision {
    let Some(row) = find(rows, call.name) else {
        return Decision::new(Tier::Never, Why::Unknown);
    };
    if row.min_tier == Tier::Never || has_never_value(row, call) {
        return Decision::new(Tier::Never, Why::NeverValue);
    }
    if let Some(source) = steered(row, call) {
        return Decision::new(Tier::Ask, Why::Steered(source));
    }
    if let Some(source) = outbound(row, call) {
        return Decision::new(Tier::Ask, Why::Outbound(source));
    }
    if row.facts.read_only {
        if !row.facts.open_world {
            return Decision::new(derive(row).max(Tier::Run), Why::Read);
        }
        return match hosts_grounded(words, row, call) {
            true => Decision::new(row.min_tier.max(Tier::Run), Why::Grounded),
            false => Decision::new(Tier::Ask, Why::Ungrounded),
        };
    }
    if !english {
        return Decision::new(Tier::Ask, Why::NotEnglish);
    }
    let asked = matches!(cued(words, row), Some(false)) && arguments_in_line(words, row, call);
    match (asked, derive(row)) {
        (true, Tier::Run) => Decision::new(Tier::Run, Why::Asked),
        (true, tier) => Decision::new(tier, Why::NoUndo),
        (false, tier) => Decision::new(tier.max(Tier::Ask), Why::Added),
    }
}

fn has_never_value(row: &Action, call: &Call) -> bool {
    row.args.iter().any(|arg| {
        value_of(call, arg.name)
            .is_some_and(|value| arg.never.iter().any(|never| fold_eq(never, value.text)))
    })
}

/// P-T: content in the target or payload of a call with no undo or a held send, or
/// deciding that such a call runs.
fn steered(row: &Action, call: &Call) -> Option<&'static str> {
    if !matches!(row.inverse, Inverse::None(_) | Inverse::Hold { .. }) {
        return None;
    }
    if let Label::Content(source) = call.control {
        return Some(source);
    }
    row.args
        .iter()
        .filter(|arg| matches!(arg.role, Role::Target | Role::Payload))
        .find_map(|arg| match value_of(call, arg.name)?.label {
            Label::Content(source) => Some(source),
            _ => None,
        })
}

/// P-F: content as the payload, or the target, of a call that leaves the machine.
fn outbound(row: &Action, call: &Call) -> Option<&'static str> {
    if !row.facts.open_world {
        return None;
    }
    call.args.iter().find_map(|(_, value)| match value.label {
        Label::Content(source) => Some(source),
        _ => None,
    })
}

fn steered_at_all(call: &Call) -> bool {
    matches!(call.control, Label::Content(_))
        || call
            .args
            .iter()
            .any(|(_, value)| matches!(value.label, Label::Content(_)))
}

fn value_of<'a>(call: &Call<'a>, name: &str) -> Option<Value<'a>> {
    call.args
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
}

/// Whether a cue of `row` starts a word of the line: `Some(false)` if some do and
/// none is negated, `Some(true)` if any is negated, `None` if none does. A negated
/// cue wins over the others, so "print the policy, but don't switch to round robin"
/// does not ask for a switch.
fn cued(words: &[Word], row: &Action) -> Option<bool> {
    let mut found = None;
    for cue in row.cues {
        for at in matches(words, cue, true) {
            let reach = at.saturating_sub(NEGATION_REACH)..at;
            if words[reach]
                .iter()
                .any(|w| NEGATIONS.contains(&w.text.as_str()))
            {
                return Some(true);
            }
            found = Some(false);
        }
    }
    found
}

/// Every `from_request` argument is literal in the line, an alias it uses, or a
/// symbol the line points at.
fn arguments_in_line(words: &[Word], row: &Action, call: &Call) -> bool {
    row.args.iter().filter(|arg| arg.from_request).all(|arg| {
        let Some(value) = value_of(call, arg.name) else {
            // An optional argument left out asks for nothing.
            return matches!(arg.ty, ArgType::Opt(_));
        };
        if matches!(value.label, Label::Content(_)) {
            return false;
        }
        (names_a_thing(value.text) && !matches(words, value.text, false).is_empty())
            || arg.aliases.iter().any(|(written, means)| {
                fold_eq(means, value.text) && !matches(words, written, false).is_empty()
            })
            || (SYMBOLS.contains(&value.text)
                && words.iter().any(|w| POINTERS.contains(&w.text.as_str())))
    })
}

/// Every host argument is typed in the line, or is what the host table gives for a
/// word of it, and none came from content.
fn hosts_grounded(words: &[Word], row: &Action, call: &Call) -> bool {
    row.args
        .iter()
        .filter(|arg| matches!(arg.ty, ArgType::Host))
        .all(|arg| {
            value_of(call, arg.name).is_some_and(|value| {
                !matches!(value.label, Label::Content(_))
                    && ((names_a_thing(value.text)
                        && !matches(words, value.text, false).is_empty())
                        || HOSTS.iter().any(|(word, host)| {
                            *host == value.text && words.iter().any(|w| w.text == *word)
                        }))
            })
        })
}

/// Words that state a want or a complaint about how the system feels: the only
/// lines the vague path reads as asking for a change.
const WANT_WORDS: &[&str] = &[
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
const WANT_PHRASES: &[&str] = &[
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
const REPEATS: &[&str] = &["do it again", "do that again"];

/// Whether the line states a want or a complaint ([`WANT_WORDS`], [`WANT_PHRASES`]).
fn wants(words: &[Word]) -> bool {
    words.iter().any(|w| WANT_WORDS.contains(&w.text.as_str()))
        || WANT_PHRASES
            .iter()
            .any(|phrase| !matches(words, phrase, false).is_empty())
}

/// Words that start a question in English.
const QUESTION_WORDS: &[&str] = &[
    "how", "what", "which", "who", "whom", "whose", "when", "where", "why", "is", "are", "was",
    "were", "does", "do", "did", "can", "could", "will", "would", "should", "has", "have",
];

/// Whether `line` is a question: it ends with `?`, or starts with a question word
/// ("how many CPUs are there", "is the network up").
fn is_question(line: &str) -> bool {
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

/// Whether `text` names a thing: some word of it is neither a kind word nor a
/// determiner. "notes" and "the llm service" do; "the service" and "it" do not.
fn names_a_thing(text: &str) -> bool {
    words(text).iter().any(|w| {
        let w = w.text.as_str();
        !KIND_WORDS.contains(&w) && !DETERMINERS.contains(&w)
    })
}

/// One word of the line, lower case.
#[derive(Debug)]
struct Word {
    text: alloc::string::String,
}

/// The line's words: letters, digits and apostrophes, lower case. Everything else
/// separates, so "round-robin", "round robin" and "Round Robin." read alike.
fn words(text: &str) -> Vec<Word> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|w| w.trim_matches('\''))
        .filter(|w| !w.is_empty())
        .map(|w| Word {
            text: w.to_lowercase(),
        })
        .collect()
}

/// Where `phrase`'s words appear in a row in `words`. With `prefix`, its last word
/// need only start a word of the line, as a cue does ("polic" in "policy").
fn matches(words: &[Word], phrase: &str, prefix: bool) -> Vec<usize> {
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

fn fold_eq(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.text == y.text)
}

/// v0.1's English check: no letter outside the Latin alphabet, and at least half
/// the words in the shipped word list or written like code (digits, a dot).
pub fn is_english_line(line: &str) -> bool {
    is_english(line)
}

fn is_english(line: &str) -> bool {
    if line
        .chars()
        .any(|c| c.is_alphabetic() && !c.is_ascii_alphabetic())
    {
        return false;
    }
    // Whole tokens here, so `report.txt` or `build-3` counts once, as code.
    let tokens: Vec<&str> = line
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|t| !t.is_empty())
        .collect();
    let known = tokens.iter().filter(|t| known_token(t)).count();
    tokens.len() < 3 || known * 2 >= tokens.len()
}

fn known_token(token: &str) -> bool {
    let word = token.to_ascii_lowercase();
    let word = word.trim_matches('\'');
    token.contains(['.', '/', '_', '-', ':', '@'])
        || token.chars().any(|c| c.is_ascii_digit())
        || ENGLISH.binary_search(&word).is_ok()
        || ENGLISH_STEMS.iter().any(|stem| word.starts_with(stem))
}

/// Common English words, sorted. A line needs half its words here (or among the
/// stems) to count as English; names and numbers in it are the other half.
const ENGLISH: &[&str] = &[
    "a",
    "about",
    "add",
    "after",
    "again",
    "all",
    "also",
    "an",
    "and",
    "any",
    "apt",
    "are",
    "as",
    "at",
    "back",
    "be",
    "been",
    "before",
    "below",
    "brew",
    "but",
    "by",
    "call",
    "can",
    "change",
    "check",
    "clean",
    "close",
    "could",
    "current",
    "dnf",
    "do",
    "does",
    "doesn't",
    "don't",
    "down",
    "each",
    "else",
    "every",
    "feel",
    "few",
    "fewer",
    "first",
    "for",
    "free",
    "from",
    "get",
    "give",
    "go",
    "has",
    "have",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "jot",
    "just",
    "keep",
    "last",
    "less",
    "let",
    "like",
    "line",
    "list",
    "long",
    "look",
    "make",
    "many",
    "me",
    "more",
    "most",
    "move",
    "much",
    "my",
    "name",
    "names",
    "new",
    "next",
    "no",
    "not",
    "now",
    "number",
    "of",
    "off",
    "old",
    "on",
    "one",
    "only",
    "open",
    "or",
    "other",
    "out",
    "over",
    "pacman",
    "please",
    "print",
    "pull",
    "put",
    "remove",
    "run",
    "same",
    "second",
    "see",
    "set",
    "show",
    "so",
    "some",
    "something",
    "start",
    "stop",
    "sudo",
    "sure",
    "switch",
    "take",
    "tell",
    "than",
    "thanks",
    "that",
    "the",
    "their",
    "them",
    "then",
    "there",
    "these",
    "things",
    "this",
    "three",
    "tidy",
    "to",
    "too",
    "total",
    "turn",
    "two",
    "up",
    "use",
    "used",
    "using",
    "was",
    "way",
    "what",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whole",
    "why",
    "will",
    "winget",
    "with",
    "would",
    "you",
    "your",
];

/// Stems of English words the OS's requests use; a word starting with one counts.
const ENGLISH_STEMS: &[&str] = &[
    "answer",
    "app",
    "array",
    "browser",
    "char",
    "comput",
    "context",
    "count",
    "cpu",
    "decimal",
    "digit",
    "disk",
    "edit",
    "email",
    "eject",
    "fals",
    "file",
    "format",
    "heap",
    "high",
    "instal",
    "interactiv",
    "kernel",
    "length",
    "letter",
    "low",
    "map",
    "memor",
    "migrat",
    "minute",
    "model",
    "modul",
    "note",
    "packag",
    "percent",
    "pid",
    "polic",
    "process",
    "ratio",
    "read",
    "reboot",
    "restart",
    "robin",
    "round",
    "rule",
    "schedul",
    "second",
    "servic",
    "snapp",
    "sum",
    "system",
    "thread",
    "time",
    "true",
    "upper",
    "uptime",
    "version",
    "watchdog",
    "websit",
    "word",
    "yes",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn do_it_again_is_an_order_and_do_you_a_question() {
        for line in ["do it again", "Do that again please"] {
            assert!(!is_question(line), "{line}");
            assert!(wants(&words(line)), "{line}");
        }
        for line in ["do you use round robin", "do it again?", "does it run"] {
            assert!(is_question(line), "{line}");
        }
    }

    #[test]
    fn the_word_lists_are_sorted_for_binary_search() {
        assert!(ENGLISH.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn cues_start_words_and_negation_reaches_three_words() {
        let set = crate::SET_POLICY;
        assert_eq!(
            cued(&words("Switch the scheduler to round robin."), &set),
            Some(false)
        );
        assert_eq!(cued(&words("policy please"), &set), Some(false));
        assert_eq!(cued(&words("don't touch the policy"), &set), Some(true));
        assert_eq!(cued(&words("no, not the scheduler"), &set), Some(true));
        assert_eq!(
            cued(&words("not now; later, switch the policy"), &set),
            Some(false)
        );
        assert_eq!(cued(&words("apolicy"), &set), None);
        assert_eq!(cued(&words("how many cpus"), &set), None);
    }

    #[test]
    fn phrases_match_whole_words_across_spaces_and_hyphens() {
        let line = words("Use Round Robin, then open en.wikipedia.org");
        assert_eq!(matches(&line, "round-robin", false), [1]);
        assert_eq!(matches(&line, "en.wikipedia.org", false), [5]);
        assert!(matches(&line, "robin-round", false).is_empty());
        assert!(matches(&line, "roun", false).is_empty());
        assert_eq!(matches(&line, "roun", true), [1]);
    }

    #[test]
    fn lines_not_in_english_are_told_apart() {
        for line in [
            "Switch the scheduler to round robin.",
            "make it snappier",
            "install notes",
            "Is the second service called llm? Answer yes or no.",
        ] {
            assert!(is_english_line(line), "{line}");
        }
        for line in [
            "cambia la política a round robin",
            "passe le planificateur en round robin",
            "stell den Planer auf round robin um",
            "ラウンドロビンに切り替えて",
            "переключи планировщик",
        ] {
            assert!(!is_english_line(line), "{line}");
        }
    }
}
