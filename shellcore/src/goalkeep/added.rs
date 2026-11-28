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
//!   that are not pdfs into notes/other") too. But one of the negations this
//!   pass adds that only says how a folder is keeps nothing off: "isn't",
//!   "aren't" or "wasn't", or "neither ... is", then a state word
//!   ([`STATES`]): "notes/q3 isn't sorted yet, sort it", "neither notes/q3
//!   nor notes/q4 is sorted yet" (#1650's review). Main keeps those goals too.
//!   It is decided for the whole line first ([`freed`]): the state word ends
//!   its clause ("isn't ready to be touched" refuses), nothing but what it is
//!   said of comes before the negation, and every other clause is filler or
//!   a plain order on the described folders themselves ("sort it by topic",
//!   "so sort both"): it points back to them or names them all, names no
//!   other path, and selects nothing by difference ("the rest", "everything
//!   else", "the others", "sort notes/q4"). Any
//!   other line ("..., hands off", "..., nobody touches it", "..., sort it
//!   later", "my finals aren't done, notes/q3 included, so leave them") is
//!   read exactly as without the release, naming the folder it named, and
//!   its describing clause's folder is refused even where that reading found
//!   nothing ("nobody touches notes/q3 as it isn't sorted") (#1684's
//!   review).
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
//!   one it holds, so the person names its path or rewords. So does one that
//!   is a negation's subject: "the q3 folder isn't to be touched". ("don't
//!   touch the q3 folder" is refused before this check: "folder" is a row's
//!   cue, and the goal's rows refuse a negated cue.)
//!
//! A negation reaches past a comma only into a list of paths ("don't touch the
//! finals, notes/q3 or notes/q4", "don't touch the finals, the drafts or
//! notes/q3": a list word right before a path, unless the item opens with a
//! row's cue, an order: "the new drafts or notes/q3" is still the list,
//! "trash notes/old" is not), item by item past an Oxford comma ("the
//! finals, the drafts, or notes/q3")
//! or when nothing but filler follows it ("don't, under any circumstances,
//! touch notes/q3"). A negated clause that points back ("it", "them", "there")
//! keeps off the path of a clause before it that held only that path:
//! "notes/q3: don't touch it".
//!
//! Main's false refusals stay ("notes/q3: don't worry about it, just sort it",
//! "don't worry, notes/q3 is backed up, sort it", "move the files that are not
//! pdfs into notes/other", "leave a note in notes/q3 to be read later"): its
//! reader refuses them, and this pass can only add to what it refuses. A
//! reworded line costs less than a kept-off folder in the goal.
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

/// The negations #1650 added that only say how a folder is when a state word
/// follows: "notes/q3 isn't sorted yet". Main's own ("not") are left as main
/// reads them.
const COPULAR_NEGATIONS: &[&str] = &["isnt", "isn't", "arent", "aren't", "wasnt", "wasn't"];

/// Words that say how a folder is, not what is done to it: after one of
/// [`COPULAR_NEGATIONS`], or after "neither ... is", the negation keeps
/// nothing off. A word that could name the goal's own change ("touched",
/// "moved", "renamed", "archived") is not one.
const STATES: &[&str] = &[
    "sorted",
    "done",
    "finished",
    "ready",
    "complete",
    "completed",
    "empty",
    "tidy",
    "tidied",
    "organized",
    "organised",
    "clean",
    "full",
    "backed",
    "synced",
    "tagged",
    "labelled",
    "labeled",
];

/// Words that may stand between such a negation and its state word:
/// "isn't yet sorted".
const STATE_FILLER: &[&str] = &["yet", "quite", "fully", "all", "really", "properly"];

/// Verbs that open an order a [`freed`] line may go on with: "...,
/// sort it by topic". A clause that opens any other way may keep the folder
/// off, so the line is refused.
const ORDER_VERBS: &[&str] = &[
    "sort", "trash", "move", "rename", "tidy", "organize", "organise", "file", "tag", "group",
    "list", "read", "copy", "clean", "archive", "label", "merge",
];

/// Words that, in an order after a [`freed`] clause, may hold something
/// back ("..., copy nothing", "..., sort it later"), or select by difference
/// and so leave the described folder out ("..., sort the rest of notes/",
/// "everything else", "the others", "what's left", "minus it", "instead",
/// #1684's review).
const HOLDS_BACK: &[&str] = &[
    "nothing",
    "no",
    "none",
    "nobody",
    "noone",
    "alone",
    "off",
    "out",
    "away",
    "limits",
    "without",
    "later",
    "wait",
    "only",
    "untouched",
    "unchanged",
    "as",
    "rest",
    "remaining",
    "remainder",
    "remains",
    "else",
    "others",
    "other",
    "left",
    "leftover",
    "leftovers",
    "minus",
    "bar",
    "sans",
    "instead",
    "less",
    "besides",
    "outside",
    "beyond",
    "siblings",
    "sibling",
    "around",
    "neighbours",
    "neighbors",
    "next",
    "near",
    "beside",
];

/// Words that may lead a list item before a row's cue that ends the list:
/// "don't touch the finals, just trash notes/old".
const ORDER_LEADS: &[&str] = &["also", "now", "so", "and", "then"];

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
    // Decided for the whole line first: a line [`freed`] does not let
    // through reads exactly as without the release, naming what it named.
    let freed = freed(&clauses, &starts_order);
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
        if let Some(path) = in_clause(words, freed) {
            return Some(path);
        }
        // A negation that only says how a folder is still carries a list on
        // ("my finals aren't done, notes/q3 included"), as #1650 read it.
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
        } else if carried == Carry::Open
            || (carried == Carry::List && list_item(words, &starts_order))
        {
            // The order a negation before it negates, or an item of its list
            // with no path ("the drafts," before "or notes/q3", an Oxford
            // comma): the list may go on.
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
    // A line the release does not let through keeps the folder of its
    // describing clause off too, even where #1650 read nothing ("nobody
    // touches notes/q3 as it isn't sorted"): only adds a refusal.
    match freed {
        true => None,
        false => described(&clauses),
    }
}

/// Whether `words`, after a comma, go on a list of paths: "notes/q3 or
/// notes/q4", "or notes/q4", "the drafts or notes/q4" (a list word right
/// before a path, with no row's cue ahead of it that leads its item: "trash
/// notes/old" is an order, "the new drafts or notes/q3" is still the list).
fn listed(words: &[Word], starts_order: impl Fn(&str) -> bool) -> bool {
    if words.first().is_some_and(|w| w.path) {
        return true;
    }
    if ordered(words, &starts_order) {
        return false;
    }
    for pair in words.windows(2) {
        if LIST_WORDS.contains(&pair[0].text.as_str()) && pair[1].path {
            return true;
        }
    }
    false
}

/// Whether `words` open with an order: a row's cue with only filler before
/// it ("trash notes/old", "just trash notes/old").
fn ordered(words: &[Word], starts_order: impl Fn(&str) -> bool) -> bool {
    words
        .iter()
        .find(|w| {
            !CARRY_FILLER.contains(&w.text.as_str()) && !ORDER_LEADS.contains(&w.text.as_str())
        })
        .is_some_and(|w| starts_order(&w.text))
}

/// Whether `words`, after a comma in a list, are one more item of it with
/// no path: no order, no path, no word that keeps off ("the drafts").
fn list_item(words: &[Word], starts_order: impl Fn(&str) -> bool) -> bool {
    !words.is_empty()
        && !ordered(words, &starts_order)
        && words
            .iter()
            .all(|w| !w.path && !is_negation(&w.text) && !EXCLUSIONS.contains(&w.text.as_str()))
}

/// Whether the negation at `at` only says how a folder is: one of
/// [`COPULAR_NEGATIONS`], or "neither" and its "is", "are" or "was", then a
/// [`STATES`] word ("notes/q3 isn't sorted yet", "neither notes/q3 nor
/// notes/q4 is sorted").
fn describes(words: &[Word], at: usize) -> bool {
    let word = words[at].text.as_str();
    let from = match word {
        _ if COPULAR_NEGATIONS.contains(&word) => at + 1,
        "neither" => match words[at + 1..]
            .iter()
            .position(|w| matches!(w.text.as_str(), "is" | "are" | "was"))
        {
            Some(copula) => at + 2 + copula,
            None => return false,
        },
        _ => return false,
    };
    // The state word ends the clause, but for filler: "isn't ready to be
    // touched" is no state (#1683's review).
    let mut rest = words[from.min(words.len())..]
        .iter()
        .filter(|w| !STATE_FILLER.contains(&w.text.as_str()));
    rest.next()
        .is_some_and(|w| STATES.contains(&w.text.as_str()))
        && rest.all(|w| w.text == "up")
}

/// Whether `clauses` are a line the release lets through: a clause whose
/// negation only says how a folder is ([`describes`]) holds nothing before
/// that negation but what it is said of, and every other clause is filler or
/// a plain order on the described folders (it opens with a row's cue or one
/// of [`ORDER_VERBS`], points back to them or names them all, names no other
/// path, and has no negation, exclusion or [`HOLDS_BACK`] word): "notes/q3
/// isn't sorted yet, sort it by topic". Any other line ("..., hands off", "..., nobody
/// touches it", "..., so wait", "nobody touches notes/q3 as it isn't
/// sorted") is read as #1650 read it, and refused as it refused it, naming
/// the same folder (#1684's review).
fn freed(clauses: &[Clause], starts_order: impl Fn(&str) -> bool) -> bool {
    let Some(at) = clauses.iter().position(|c| {
        (0..c.words.len()).any(|i| is_negation(&c.words[i].text) && describes(&c.words, i))
    }) else {
        return false;
    };
    let words = &clauses[at].words;
    let Some(i) = (0..words.len()).find(|&i| is_negation(&words[i].text) && describes(words, i))
    else {
        return false;
    };
    // Before the negation, only what it is said of: "notes/q3 and notes/q4",
    // "neither notes/q3 nor notes/q4 is".
    let only_its_subject = words[..i].iter().all(|w| {
        w.path
            || LIST_WORDS.contains(&w.text.as_str())
            || COPULAS.contains(&w.text.as_str())
            || TOPIC_FILLER.contains(&w.text.as_str())
    });
    // Every other clause is filler, or an order on the described folders
    // themselves: it points back to them ("sort it", "sort both") or names
    // them, names no other path ("sort notes/q4" would leave notes/q3 a
    // folder the goal was only told about), and has no word that may hold
    // something back or select by difference ("copy nothing", "sort it
    // later", "sort the rest of notes/", #1684's review).
    let described: Vec<&str> = words
        .iter()
        .filter(|w| w.path)
        .map(|w| w.text.as_str())
        .collect();
    let order = |c: &Clause| {
        // To every described folder: with two or more, a plural pointer or
        // each by name ("sort notes/q4" or "sort it" after "notes/q3 and
        // notes/q4 aren't sorted" picks one, by difference).
        let plural = c
            .words
            .iter()
            .any(|w| matches!(w.text.as_str(), "them" | "both" | "these" | "those" | "all"));
        let singular = c
            .words
            .iter()
            .any(|w| matches!(w.text.as_str(), "it" | "this" | "that" | "there"));
        let all_named = described
            .iter()
            .all(|d| c.words.iter().any(|w| w.text == *d));
        let refers_back = all_named || plural || (singular && described.len() == 1);
        let no_other_path = c
            .words
            .iter()
            .all(|w| !w.path || described.contains(&w.text.as_str()));
        ordered(&c.words, |w: &str| {
            starts_order(w) || ORDER_VERBS.contains(&w)
        }) && refers_back
            && no_other_path
            && !c
                .words
                .iter()
                .any(|w| HOLDS_BACK.contains(&w.text.as_str()) || keeps_off_word(&w.text))
    };
    only_its_subject
        && clauses
            .iter()
            .enumerate()
            .all(|(j, c)| j == at || c.words.iter().all(filler) || order(c))
}

/// The folder of the first clause whose negation only says how a folder is
/// ([`describes`]): the first path after the negation, else its subject,
/// the last path before it, else a child folder by its bare name.
fn described(clauses: &[Clause]) -> Option<String> {
    clauses.iter().find_map(|c| {
        let words = &c.words;
        let i = (0..words.len()).find(|&i| is_negation(&words[i].text) && describes(words, i))?;
        words[i + 1..]
            .iter()
            .find(|w| w.path)
            .or_else(|| words[..i].iter().rev().find(|w| w.path))
            .map(|w| w.text.clone())
            .or_else(|| folder_subject(&words[..i]))
    })
}

/// The first path one clause keeps off on its own.
fn in_clause(words: &[Word], freed: bool) -> Option<String> {
    let text = |i: usize| words.get(i).map_or("", |w| w.text.as_str());
    let to_destination = |from: usize| {
        words
            .iter()
            .skip(from)
            .take_while(|w| !DESTINATIONS.contains(&w.text.as_str()))
    };
    for (i, word) in words.iter().enumerate() {
        // In a line the release lets through, a negation that only says how
        // a folder is keeps nothing off.
        if is_negation(&word.text) && !(freed && describes(words, i)) {
            if let Some(path) = words[i + 1..].iter().find(|w| w.path) {
                return Some(path.text.clone());
            }
            // Its subject: a path before it with only copulas between.
            if let Some(path) = subject(&words[..i]) {
                return Some(path);
            }
            // "the q3 folder isn't to be touched".
            if let Some(name) = folder_subject(&words[..i]) {
                return Some(name);
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

/// The bare name of a child folder that ends `before`, the words ahead of a
/// negation, with only copulas after it: "the q3 folder is not".
fn folder_subject(before: &[Word]) -> Option<String> {
    let at = before
        .iter()
        .rposition(|w| matches!(w.text.as_str(), "folder" | "folders"))?;
    let named = before[at + 1..]
        .iter()
        .all(|w| COPULAS.contains(&w.text.as_str()));
    named.then(|| folder_name(before[at.saturating_sub(2)..=at].iter()))?
}

/// Whether `word` is a negation, an exclusion or a word of one of
/// [`EXCLUSION_PAIRS`].
fn keeps_off_word(word: &str) -> bool {
    is_negation(word)
        || EXCLUSIONS.contains(&word)
        || EXCLUSION_PAIRS
            .iter()
            .any(|(a, b)| word == *a || word == *b)
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
