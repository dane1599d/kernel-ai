//! A folder a goal's line says not to touch (#1577's review). A goal works only
//! in the folders its person named for it, so a path the line keeps off ("don't
//! touch notes/q3", "everything but notes/q3", "leave notes/q3 alone") refuses
//! the goal, and the refusal names that path. A negation that is about
//! something else ("don't touch the finals but trash the drafts in notes/old",
//! "move everything except the pdfs into notes/archive") keeps the goal.
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
//!   "notes/q3 is off limits". A negation of worry ([`REASSURANCES`]: "don't
//!   worry", "never mind") keeps nothing off when no path, bare folder or
//!   pointer follows it in its clause; "don't bother with notes/q3" keeps it
//!   off, and so does "notes/q3: don't worry about it" unless a later clause
//!   gives an order about the same pointer ("..., just sort it"). In a
//!   relative clause, a "not" after "is", "are", "was" or "were", or an
//!   "isn't", "aren't" or "wasn't", describes files ("the files that are not
//!   pdfs"), so it reads only up to a destination; never before "to" ("you
//!   are not to touch notes/q3", "the files that are not to be touched").
//! - **An exclusion** ([`EXCLUSIONS`], "apart from", "other than", "everything
//!   but", "keep out of", "leave out" ...) keeps off the paths after it up to a
//!   destination ("into", "to", "onto"): "trash notes/ except notes/q3" keeps
//!   notes/q3 off, "move everything except the pdfs into notes/archive" keeps
//!   nothing off. So does "without" and a word ending in "ing": "sort notes/
//!   without touching notes/q3".
//! - **"leave ... alone"** (or "untouched", "unchanged") keeps off the paths
//!   between. "be", "as" and "out" end it only after a path, and "be" not
//!   right after "to": "leave notes/q3 be", "leave notes/q3 just as it is",
//!   "leave notes/q3 out", but not "leave a note in notes/q3 to be read
//!   later".
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
//! ("don't skip notes/q3"), which reads as keeping it off.

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

/// Words a negation of which keeps nothing off: "don't worry about it".
const REASSURANCES: &[&str] = &["worry", "mind", "bother", "hesitate", "forget", "panic"];

/// Copulas before which a "not" describes files instead of keeping them off:
/// "the files that are not pdfs".
const DESCRIBING: &[&str] = &["is", "are", "was", "were"];

/// Words that open the relative clause a describing "not" stands in: "the
/// files that are not pdfs".
const RELATIVES: &[&str] = &["that", "which", "who"];

/// Negations that are a copula and "not" in one: "the files that aren't pdfs".
const DESCRIBING_NOTS: &[&str] = &["isnt", "isn't", "arent", "aren't", "wasnt", "wasn't"];

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

/// Words that end "leave ...": "leave notes/q3 alone".
const LEAVE_ENDS: &[&str] = &["alone", "untouched", "unchanged"];

/// Words that end "leave ..." only after a path, and "be" not right after
/// "to": "leave notes/q3 be", "leave notes/q3 just as it is", "leave
/// notes/q3 out", but not "leave a note in notes/q3 to be read".
const LEAVE_ENDS_AFTER_PATH: &[&str] = &["be", "as", "out"];

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
    let clauses = clauses(line, &starts_order);
    let mut carry = Carry::No;
    let mut topic: Option<&str> = None;
    for (index, clause) in clauses.iter().enumerate() {
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
        let keeps_off =
            |i: usize| negates(words, i) || EXCLUSIONS.contains(&words[i].text.as_str());
        // "notes/q3: don't worry about it, just sort it": a negation of worry
        // whose pointer a later clause gives an order about keeps nothing off.
        // That clause names the pointer and nothing that keeps off: no
        // negation, exclusion, "leave" or "alone" ("..., leave it" keeps off).
        let later = &clauses[index + 1..];
        let ordered_later = later.iter().any(|c| {
            c.words.iter().any(|w| POINTERS.contains(&w.text.as_str()))
                && !c.words.iter().any(|w| {
                    let text = w.text.as_str();
                    is_negation(text)
                        || EXCLUSIONS.contains(&text)
                        || KEPT_AS_IS.contains(&text)
                        || text == "leave"
                })
        });
        let excused = |i: usize| {
            reassures(words, i)
                && ordered_later
                && !words[i + 1..].iter().any(|w| w.path)
                && folder_name(words[i + 1..].iter()).is_none()
        };
        if let Some(at) = (0..words.len()).find(|&i| keeps_off(i) && !excused(i)) {
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
        if negates(words, i) {
            // Its subject: a path before it with only copulas between.
            if let Some(path) = subject(&words[..i]) {
                return Some(path);
            }
            // A "not" that describes files reads only up to a destination:
            // "move the files that are not pdfs into notes/other".
            let found = match describes(words, i) {
                true => to_destination(i + 1).find(|w| w.path),
                false => words[i + 1..].iter().find(|w| w.path),
            };
            if let Some(path) = found {
                return Some(path.text.clone());
            }
            if let Some(name) = folder_name(to_destination(i + 1)) {
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
            let end = words[i + 1..].iter().enumerate().position(|(j, w)| {
                LEAVE_ENDS.contains(&w.text.as_str())
                    || (LEAVE_ENDS_AFTER_PATH.contains(&w.text.as_str())
                        && words[i + 1..i + 1 + j].iter().any(|w| w.path)
                        && !(w.text == "be" && words[i + j].text == "to"))
            });
            if let Some(end) = end {
                let between = &words[i + 1..i + 1 + end];
                if let Some(path) = between.iter().find(|w| w.path) {
                    return Some(path.text.clone());
                }
                if let Some(name) = folder_name(between.iter()) {
                    return Some(name);
                }
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

/// Whether `words[at]` negates a word of worry: "don't worry", "never mind".
fn reassures(words: &[Word], at: usize) -> bool {
    is_negation(&words[at].text)
        && words
            .get(at + 1)
            .is_some_and(|next| REASSURANCES.contains(&next.text.as_str()))
}

/// Whether `words[at]` is a negation that keeps something off: a negation
/// of worry only when a path, a folder's bare name or a pointer follows it
/// in its clause ("don't bother with notes/q3", "don't worry about it"),
/// never a bare "don't worry".
fn negates(words: &[Word], at: usize) -> bool {
    if !is_negation(&words[at].text) {
        return false;
    }
    let rest = &words[at + 1..];
    !reassures(words, at)
        || rest
            .iter()
            .any(|w| w.path || POINTERS.contains(&w.text.as_str()))
        || folder_name(rest.iter()).is_some()
}

/// Whether the negation at `words[at]` describes files in a relative clause
/// ("the files that are not pdfs", "which aren't pdfs") instead of keeping
/// them off. Never before "to": "you are not to touch", "the files that are
/// not to be touched".
fn describes(words: &[Word], at: usize) -> bool {
    let text = |i: usize| words.get(i).map_or("", |w| w.text.as_str());
    let word = text(at);
    let relative = |i: Option<usize>| i.is_some_and(|i| RELATIVES.contains(&text(i)));
    let described = match word {
        "not" => DESCRIBING.contains(&text(at.wrapping_sub(1))) && relative(at.checked_sub(2)),
        _ => DESCRIBING_NOTS.contains(&word) && relative(at.checked_sub(1)),
    };
    described && text(at + 1) != "to"
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
