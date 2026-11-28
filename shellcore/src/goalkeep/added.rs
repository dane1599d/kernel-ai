//! #1650's additions to the kept-off check, read on their own after
//! [`super::main_pass`]: a line either pass refuses is refused, so nothing
//! here can let through a line main refused. It is main's reader with the
//! additions below.
//!
//! Read in clauses. A clause ends at `,` `;` `:` `!` `?` or a sentence's `.`,
//! and at `,` `;` `:` `!` `?` inside a word that is not a path ("the
//! drafts,don't touch notes/q3"), and before "but", "then", "instead",
//! "however", "though", "although", "while" or "whereas", and before an "and"
//! that starts another order ("and trash ...", "and don't ..."). In a clause:
//!
//! - **A negation** ([`NEGATIONS`]) keeps off every path after it: "don't touch
//!   notes/q3", "never move anything in notes/q3", "neither notes/q3 nor
//!   notes/q4". So does one whose path is its subject: "notes/q3 is not to be
//!   touched", "notes/q3 isn't to be touched", "notes/q3 stays untouched",
//!   "notes/q3 is off limits". Every negation counts, a negation of worry
//!   ("don't bother with notes/q3") and one that describes files ("the files
//!   that are not pdfs into notes/other") too.
//! - **An exclusion** ([`EXCLUSIONS`], "apart from", "other than", "everything
//!   but", "keep out of", "leave out" ...) keeps off the paths after it up to a
//!   destination ("into", "to", "onto"): "trash notes/ except notes/q3" keeps
//!   notes/q3 off, "move everything except the pdfs into notes/archive" keeps
//!   nothing off. So does "without" and a word ending in "ing": "sort notes/
//!   without touching notes/q3".
//! - **"leave ... alone"** (or "untouched", "unchanged", "be", "as" anywhere
//!   after) keeps off the paths between: "leave notes/q3 just as it is",
//!   "leave notes/q3 to be sorted later". So does "out" right after a path:
//!   "leave notes/q3 out".
//! - **A child folder by its bare name** after a negation or an exclusion, or
//!   in "leave ... alone" ("except the q3 folder", "leave my q3 folder
//!   alone"), refuses too, naming "q3": a goal cannot leave a folder out of
//!   one it holds, so the person names its path or rewords. ("don't touch the
//!   q3 folder" is refused before this check: "folder" is a row's cue, and
//!   the goal's rows refuse a negated cue.)
//!
//! A negation reaches past a comma only into a list of paths ("don't touch the
//! finals, notes/q3 or notes/q4", "don't touch the finals, the drafts or
//! notes/q3": a list word right before a path, with no row's cue ahead of it)
//! or when nothing but filler follows it ("don't, under any circumstances,
//! touch notes/q3"). A negated clause that points back ("it", "them", "there")
//! keeps off the path of a clause before it that held only that path:
//! "notes/q3: don't touch it".
//!
//! Apostrophe look-alikes (`’` `‘` `ʼ` `` ` `` `´` `′` `＇` `ʹ` `՚`) read as
//! `'`, the same marks agentabi's consent words fold.
//!
//! Not read, so a goal is not refused for them (its card still lists its
//! folders before it starts): a word that keeps a folder as it is without
//! negating ("keep notes/q3 as it is", "keep notes/q3 private", "notes/q3 is
//! final", "protect notes/q3", "spare notes/q3", "refrain from"); "no", and
//! "without" before anything but an "-ing" word, which describe files ("the
//! files with no tag"), so "no changes to notes/q3" is not read; "only"
//! ("only trash notes/old" says nothing of other folders); a child folder
//! named any other way ("the folder called q3", "the drafts in q3"); a negation in one clause and
//! its path in another without a list, filler or a pointer ("don't touch
//! anything. notes/q3 is where the finals are"); a path before its negation
//! with more than "is", "must", "should" ... between ("notes/q3, which has the
//! finals, must not change"); a pointer back to a clause that said more than
//! its path ("sort notes/q3, but don't rename it"); and double negation
//! ("don't skip notes/q3"), which reads as keeping it off. Main missed these
//! too: "no need to worry about notes/q3", "leave notes/q3 the way it is",
//! and a negation alone in the clause after its path ("trash the drafts in
//! notes/, notes/q3, don't bother", "notes/q3: never mind").

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
    "isnt",
    "isn't",
    "arent",
    "aren't",
    "wasnt",
    "wasn't",
    "shant",
    "shan't",
    "neither",
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
    ("leave", "out"),
];

/// Words before a folder's bare name: "the q3 folder", "my q3 folder".
const FOLDER_OWNERS: &[&str] = &["the", "my", "your", "our"];

/// Words between "the" and "folder" that name no folder: "the new folder".
const NO_FOLDER_NAME: &[&str] = &[
    "new", "other", "same", "one", "empty", "own", "parent", "whole", "right", "wrong", "next",
    "last", "first", "main", "top",
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
pub(super) fn kept_off(line: &str, starts_order: impl Fn(&str) -> bool) -> Option<String> {
    let clauses = clauses(line, &starts_order);
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
            Carry::List if listed(words, &starts_order) => return first_path,
            _ => {}
        }
        if let Some(path) = in_clause(words) {
            return Some(path);
        }
        let keeps_off = |w: &Word| is_negation(&w.text) || EXCLUSIONS.contains(&w.text.as_str());
        // A pointer after the first word that keeps off points back; whether
        // only filler follows is read after the last, so a negation main did
        // not know ("the backup isn't done so don't, under any circumstances,
        // touch notes/q3") never turns its Open carry into a List (#1650's
        // review).
        if let Some(first) = words.iter().position(keeps_off) {
            let last = words.iter().rposition(keeps_off).unwrap_or(first);
            let points_back = words[first + 1..]
                .iter()
                .any(|w| POINTERS.contains(&w.text.as_str()));
            match (topic, points_back) {
                (Some(path), true) => return Some(path.to_string()),
                _ if words[last + 1..].iter().all(filler) => carry = Carry::Open,
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
/// notes/q4", "or notes/q4", "the drafts or notes/q4" (a list word right
/// before a path, with no row's cue ahead of it).
fn listed(words: &[Word], starts_order: impl Fn(&str) -> bool) -> bool {
    if words.first().is_some_and(|w| w.path) {
        return true;
    }
    for pair in words.windows(2) {
        if starts_order(&pair[0].text) {
            return false;
        }
        if LIST_WORDS.contains(&pair[0].text.as_str()) && pair[1].path {
            return true;
        }
    }
    false
}

/// The first path one clause keeps off on its own.
fn in_clause(words: &[Word]) -> Option<String> {
    let text = |i: usize| words.get(i).map_or("", |w| w.text.as_str());
    let to_destination = |from: usize| {
        words
            .iter()
            .skip(from)
            .take_while(|w| !DESTINATIONS.contains(&w.text.as_str()))
    };
    for (i, word) in words.iter().enumerate() {
        if is_negation(&word.text) {
            if let Some(path) = words[i + 1..].iter().find(|w| w.path) {
                return Some(path.text.clone());
            }
            // Its subject: a path before it with only copulas between.
            if let Some(path) = subject(&words[..i]) {
                return Some(path);
            }
            if let Some(name) = folder_name(words[i + 1..].iter()) {
                return Some(name);
            }
        }
        // "sort notes/ without touching notes/q3".
        if word.text == "without"
            && text(i + 1).len() > 4
            && text(i + 1).ends_with("ing")
            && let Some(path) = to_destination(i + 2).find(|w| w.path)
        {
            return Some(path.text.clone());
        }
        let pair = EXCLUSION_PAIRS
            .iter()
            .any(|(a, b)| word.text == *a && text(i + 1) == *b);
        let excluded = EXCLUSIONS.contains(&word.text.as_str()) || pair;
        // "but" in "everything but" is the pair's; "but not" is too.
        if excluded {
            let from = i + if pair { 2 } else { 1 };
            if let Some(path) = to_destination(from).find(|w| w.path) {
                return Some(path.text.clone());
            }
            if let Some(name) = folder_name(to_destination(from)) {
                return Some(name);
            }
        }
        if word.text == "leave" {
            let after = &words[i + 1..];
            // Main's end: the first of [`LEAVE_ENDS`] anywhere after.
            let end = after
                .iter()
                .position(|w| LEAVE_ENDS.contains(&w.text.as_str()));
            if let Some(end) = end {
                if let Some(path) = after[..end].iter().find(|w| w.path) {
                    return Some(path.text.clone());
                }
                if let Some(name) = folder_name(after[..end].iter()) {
                    return Some(name);
                }
            }
            // Added: "out" right after a path, "leave notes/q3 out".
            let out = (1..after.len()).find(|&j| after[j].text == "out" && after[j - 1].path);
            if let Some(out) = out
                && let Some(path) = after[..out].iter().find(|w| w.path)
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

/// The name in "the q3 folder", a child folder kept off by its bare name, which
/// the goal cannot leave out of a folder it holds.
fn folder_name<'a>(words: impl Iterator<Item = &'a Word>) -> Option<String> {
    let words: Vec<&Word> = words.collect();
    words.windows(3).find_map(|w| {
        let named = FOLDER_OWNERS.contains(&w[0].text.as_str())
            && !w[1].path
            && !NO_FOLDER_NAME.contains(&w[1].text.as_str())
            && matches!(w[2].text.as_str(), "folder" | "folders");
        named.then(|| w[1].text.clone())
    })
}

fn filler(word: &Word) -> bool {
    !word.path && CARRY_FILLER.contains(&word.text.as_str())
}

/// `line`'s clauses, lower case, apostrophe look-alikes read as `'`.
fn clauses(line: &str, starts_order: impl Fn(&str) -> bool) -> Vec<Clause> {
    let line: String = line
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02bc}' | '`' | '\u{00b4}' | '\u{2032}' | '\u{ff07}'
            | '\u{02b9}' | '\u{055a}' => '\'',
            c => c.to_ascii_lowercase(),
        })
        .collect();
    // Each word, and the mark after it: 0 none, 1 a comma, 2 a stronger one.
    let mut marked: Vec<(Word, u8)> = Vec::new();
    // A word that is not a path splits after an inner mark: "drafts,don't".
    let pieces = line.split_whitespace().flat_map(|token| {
        let split = !token.contains('/');
        token.split_inclusive(move |c: char| split && ",;:!?".contains(c))
    });
    for raw in pieces {
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
