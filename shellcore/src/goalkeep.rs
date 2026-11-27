//! A folder a goal's line says not to touch (#1577's review). A goal works only
//! in the folders its person named for it, so a path the line keeps off ("don't
//! touch notes/q3", "everything but notes/q3", "leave notes/q3 alone") refuses
//! the goal, and the refusal names that path. A negation that is about
//! something else ("don't touch the finals but trash the drafts in notes/old",
//! "move everything except the pdfs into notes/archive") keeps the goal.
//!
//! Read in clauses. A clause ends at `,` `;` `:` `!` `?` or a sentence's `.`, and
//! before "but", "then", "instead", "however", "though", "although", "while" or
//! "whereas", and before an "and" that starts another order ("and trash ...",
//! "and don't ..."). In a clause:
//!
//! - **A negation** ([`NEGATIONS`]) keeps off every path after it: "don't touch
//!   notes/q3", "never move anything in notes/q3". So does one whose path is
//!   its subject: "notes/q3 is not to be touched", "notes/q3 stays untouched",
//!   "notes/q3 is off limits".
//! - **An exclusion** ([`EXCLUSIONS`], "apart from", "other than", "everything
//!   but", "keep out of" ...) keeps off the paths after it up to a destination
//!   ("into", "to", "onto"): "trash notes/ except notes/q3" keeps notes/q3 off,
//!   "move everything except the pdfs into notes/archive" keeps nothing off.
//! - **"leave ... alone"** (or "untouched", "be", "as is") keeps off the paths
//!   between.
//!
//! A negation reaches past a comma only into a list of paths ("don't touch the
//! finals, notes/q3 or notes/q4") or when nothing but filler follows it ("don't,
//! under any circumstances, touch notes/q3"). A negated clause that points back
//! ("it", "them", "there") keeps off the path of a clause before it that held
//! only that path: "notes/q3: don't touch it".
//!
//! Apostrophe look-alikes (`’` `‘` `ʼ` `` ` `` `´` `′`) read as `'`.
//!
//! Not read, so a goal is not refused for them (its card still lists its
//! folders before it starts): a word that keeps a folder as it is without
//! negating ("keep notes/q3 as it is", "notes/q3 is final", "protect notes/q3",
//! "spare notes/q3", "refrain from"); "no" and "without", which describe files
//! ("the files with no tag"), so "no changes to notes/q3" is not read; "only"
//! ("only trash notes/old" says nothing of other folders); a negation in one
//! clause and its path in another without a list, filler or a pointer
//! ("don't touch anything. notes/q3 is where the finals are"); a path before
//! its negation with more than "is", "must", "should" ... between ("notes/q3,
//! which has the finals, must not change"); a pointer back to a clause that
//! said more than its path ("sort notes/q3, but don't rename it"); and double
//! negation ("don't skip notes/q3"), which reads as keeping it off.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Words that negate what their clause does from there on.
const NEGATIONS: &[&str] = &[
    "dont",
    "don't",
    "doesnt",
    "doesn't",
    "didnt",
    "didn't",
    "not",
    "never",
    "wont",
    "won't",
    "shouldnt",
    "shouldn't",
    "mustnt",
    "mustn't",
    "cant",
    "can't",
    "cannot",
    "avoid",
    "avoiding",
];

/// Words that leave out what follows them, up to a destination.
const EXCLUSIONS: &[&str] = &[
    "except",
    "excluding",
    "exclude",
    "skip",
    "skipping",
    "ignore",
    "ignoring",
];

/// Two words that leave out what follows them: "apart from notes/q3".
const EXCLUSION_PAIRS: &[(&str, &str)] = &[
    ("apart", "from"),
    ("aside", "from"),
    ("other", "than"),
    ("but", "not"),
    ("save", "for"),
    ("everything", "but"),
    ("all", "but"),
    ("anything", "but"),
    ("keep", "out"),
    ("stay", "out"),
    ("keep", "away"),
    ("stay", "away"),
    ("hands", "off"),
];

/// Words before which an exclusion ends: where the goal puts things.
const DESTINATIONS: &[&str] = &["into", "to", "onto", "toward", "towards"];

/// Words that start a clause of their own.
const CLAUSE_WORDS: &[&str] = &[
    "but", "then", "instead", "however", "though", "although", "while", "whereas",
];

/// Words that may stand between a path and its negation: "notes/q3 is not
/// to be touched".
const COPULAS: &[&str] = &[
    "is", "are", "was", "be", "must", "should", "shall", "will", "has", "have", "to", "stays",
    "stay", "remains", "remain", "please", "folder",
];

/// Words after a path that keep it off: "notes/q3 stays untouched".
const KEPT_AS_IS: &[&str] = &["alone", "untouched", "unchanged", "off-limits"];

/// Words that end "leave ...": "leave notes/q3 alone", "leave notes/q3 be".
const LEAVE_ENDS: &[&str] = &["alone", "untouched", "unchanged", "be", "as"];

/// Words after a negation that carry it past a comma: "don't, under any
/// circumstances, touch notes/q3".
const CARRY_FILLER: &[&str] = &[
    "under",
    "any",
    "circumstances",
    "ever",
    "at",
    "all",
    "please",
    "whatever",
    "no",
    "matter",
    "what",
    "happens",
    "cost",
    "costs",
    "even",
    "if",
    "you",
    "can",
    "really",
    "just",
    "kai",
];

/// Words that point back to a clause before: "notes/q3: don't touch it".
const POINTERS: &[&str] = &["it", "them", "there", "that", "those", "these", "this"];

/// Words that may stand beside a path that is its clause's topic: "as for
/// notes/q3", "my notes/q3 folder".
const TOPIC_FILLER: &[&str] = &[
    "as",
    "for",
    "about",
    "regarding",
    "the",
    "my",
    "folder",
    "and",
];

/// Clause words that list paths after a comma: "notes/q3 or notes/q4".
const LIST_WORDS: &[&str] = &["or", "nor", "and", "either", "neither"];

struct Word {
    text: String,
    path: bool,
}

struct Clause {
    words: Vec<Word>,
    /// Whether a comma, not a stronger mark or a clause word, ended the clause
    /// before it.
    after_comma: bool,
}

/// How a clause's negation reaches past the comma after it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Carry {
    No,
    /// Into a list of paths: "don't touch the finals, notes/q3 or notes/q4".
    List,
    /// Into the next clause, its order: "don't, under any circumstances,
    /// touch notes/q3".
    Open,
}

/// The first path `line` keeps off, as the module says; `starts_order` says
/// whether a word after "and" starts another order (a row's cue, as the goal's
/// scope reads it).
pub(crate) fn kept_off(line: &str, starts_order: impl Fn(&str) -> bool) -> Option<String> {
    let clauses = clauses(line, starts_order);
    let mut carry = Carry::No;
    let mut topic: Option<&str> = None;
    for clause in &clauses {
        let words = &clause.words;
        let carried = match clause.after_comma {
            true => carry,
            false => Carry::No,
        };
        carry = Carry::No;
        let first_path = words.iter().find(|w| w.path).map(|w| w.text.clone());
        match carried {
            Carry::Open if first_path.is_some() => return first_path,
            Carry::Open if words.iter().all(filler) => {
                carry = Carry::Open;
                continue;
            }
            Carry::List if listed(words) => return first_path,
            _ => {}
        }
        if let Some(path) = in_clause(words) {
            return Some(path);
        }
        let keeps_off = |w: &Word| is_negation(&w.text) || EXCLUSIONS.contains(&w.text.as_str());
        if let Some(at) = words.iter().position(keeps_off) {
            let rest = &words[at + 1..];
            let points_back = rest.iter().any(|w| POINTERS.contains(&w.text.as_str()));
            match (topic, points_back) {
                (Some(path), true) => return Some(path.to_string()),
                _ if rest.iter().all(filler) => carry = Carry::Open,
                _ => carry = Carry::List,
            }
        } else if carried == Carry::Open {
            // The order a negation before it negates: a list may go on.
            carry = Carry::List;
        }
        topic = match words.iter().filter(|w| w.path).count() {
            1 if words
                .iter()
                .all(|w| w.path || TOPIC_FILLER.contains(&w.text.as_str())) =>
            {
                words.iter().find(|w| w.path).map(|w| w.text.as_str())
            }
            _ => None,
        };
    }
    None
}

/// Whether `words`, after a comma, go on a list of paths: "notes/q3 or
/// notes/q4", "or notes/q4".
fn listed(words: &[Word]) -> bool {
    match words {
        [first, ..] if first.path => true,
        [first, second, ..] => LIST_WORDS.contains(&first.text.as_str()) && second.path,
        _ => false,
    }
}

/// The first path one clause keeps off on its own.
fn in_clause(words: &[Word]) -> Option<String> {
    let text = |i: usize| words.get(i).map_or("", |w| w.text.as_str());
    for (i, word) in words.iter().enumerate() {
        if is_negation(&word.text) {
            if let Some(path) = words[i + 1..].iter().find(|w| w.path) {
                return Some(path.text.clone());
            }
            // Its subject: a path before it with only copulas between.
            if let Some(path) = subject(&words[..i]) {
                return Some(path);
            }
        }
        let pair = EXCLUSION_PAIRS
            .iter()
            .any(|(a, b)| word.text == *a && text(i + 1) == *b);
        let excluded = EXCLUSIONS.contains(&word.text.as_str()) || pair;
        // "but" in "everything but" is the pair's; "but not" is too.
        if excluded {
            let from = i + if pair { 2 } else { 1 };
            let found = words
                .iter()
                .skip(from)
                .take_while(|w| !DESTINATIONS.contains(&w.text.as_str()))
                .find(|w| w.path);
            if let Some(path) = found {
                return Some(path.text.clone());
            }
        }
        if word.text == "leave" {
            let end = words[i + 1..]
                .iter()
                .position(|w| LEAVE_ENDS.contains(&w.text.as_str()));
            if let Some(end) = end
                && let Some(path) = words[i + 1..i + 1 + end].iter().find(|w| w.path)
            {
                return Some(path.text.clone());
            }
        }
        let kept_as_is = KEPT_AS_IS.contains(&word.text.as_str())
            || (word.text == "off" && text(i + 1) == "limits");
        if kept_as_is && let Some(path) = subject(&words[..i]) {
            return Some(path);
        }
    }
    None
}

/// The path that ends `before`, the words ahead of a negation, with only
/// copulas after it: "notes/q3 is not".
fn subject(before: &[Word]) -> Option<String> {
    let at = before.iter().rposition(|w| w.path)?;
    before[at + 1..]
        .iter()
        .all(|w| COPULAS.contains(&w.text.as_str()))
        .then(|| before[at].text.clone())
}

fn is_negation(word: &str) -> bool {
    NEGATIONS.contains(&word)
}

fn filler(word: &Word) -> bool {
    !word.path && CARRY_FILLER.contains(&word.text.as_str())
}

/// `line`'s clauses, lower case, apostrophe look-alikes read as `'`.
fn clauses(line: &str, starts_order: impl Fn(&str) -> bool) -> Vec<Clause> {
    let line: String = line
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02bc}' | '`' | '\u{00b4}' | '\u{2032}' => '\'',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    // Each word, and the mark after it: 0 none, 1 a comma, 2 a stronger one.
    let mut marked: Vec<(Word, u8)> = Vec::new();
    for raw in line.split_whitespace() {
        let raw = raw.trim_start_matches(|c: char| "\"'(".contains(c));
        let core = raw.trim_end_matches(|c: char| ",;:!?.)\"'".contains(c));
        let tail = &raw[core.len()..];
        let mark = match tail {
            _ if tail.contains([';', ':', '!', '?', '.']) => 2,
            _ if tail.contains(',') => 1,
            _ => 0,
        };
        if core.is_empty() {
            if let Some(last) = marked.last_mut() {
                last.1 = last.1.max(mark);
            }
            continue;
        }
        let word = Word {
            text: core.to_string(),
            path: core.contains('/'),
        };
        marked.push((word, mark));
    }
    let mut out: Vec<Clause> = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut after_comma = false;
    let count = marked.len();
    let mut iter = marked.into_iter().peekable();
    let mut index = 0;
    let mut previous = String::new();
    while let Some((word, mark)) = iter.next() {
        let next = iter.peek().map(|(w, _)| w);
        let starts = match word.text.as_str() {
            "but" => {
                !matches!(previous.as_str(), "everything" | "all" | "anything")
                    && next.is_none_or(|n| n.text != "not")
            }
            "and" => next.is_some_and(|n| {
                !n.path && (is_negation(&n.text) || n.text == "then" || starts_order(&n.text))
            }),
            text => CLAUSE_WORDS.contains(&text),
        };
        if starts && !words.is_empty() {
            out.push(Clause {
                words: core::mem::take(&mut words),
                after_comma,
            });
            after_comma = false;
        }
        previous = word.text.clone();
        // A clause word itself says nothing a clause reads.
        if !(starts && matches!(word.text.as_str(), "but" | "and" | "then")) {
            words.push(word);
        }
        index += 1;
        if mark > 0 || index == count {
            out.push(Clause {
                words: core::mem::take(&mut words),
                after_comma,
            });
            after_comma = mark == 1;
        }
    }
    out
}
