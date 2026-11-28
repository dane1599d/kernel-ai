//! The kept-off check (`crate::goalkeep`, #1577's second review): a goal whose
//! line keeps a folder off is refused, and the refusal names that folder.

use alloc::string::String;

pub(super) fn kept_off_refusal(path: &str) -> String {
    alloc::format!("\"{path}\" is said not to be touched; name only the folders the goal works in")
}

/// #1577's second review: the kept-off check reads each phrasing a person
/// keeps a folder off with, names that folder, and leaves a goal whose
/// negation is about something else.
#[test]
fn a_folder_kept_off_is_named_and_a_negation_of_something_else_keeps_the_goal() {
    for (line, path) in [
        (
            "leave notes/q3 alone and trash the drafts in notes/old",
            "notes/q3",
        ),
        ("trash the drafts in notes/ but skip notes/q3", "notes/q3"),
        ("skip notes/q3, trash the drafts in notes/old", "notes/q3"),
        ("trash everything in notes/ apart from notes/q3", "notes/q3"),
        ("trash everything but notes/q3", "notes/q3"),
        ("trash the drafts in notes/ other than notes/q3", "notes/q3"),
        ("trash the drafts in notes/, excluding notes/q3", "notes/q3"),
        (
            "trash the drafts in notes/old but keep out of notes/q3",
            "notes/q3",
        ),
        // Contractions, and apostrophe look-alikes.
        (
            "trash the drafts in notes/old, but don\u{2019}t touch notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old, but don\u{02bc}t touch notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old and you mustn't touch notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old but never touch notes/q3",
            "notes/q3",
        ),
        // A path before its negation.
        (
            "trash the drafts in notes/old; notes/q3 is not to be touched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; notes/q3 stays untouched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old. notes/q3 is off limits",
            "notes/q3",
        ),
        (
            "notes/q3: don't touch it. trash the drafts in notes/old",
            "notes/q3",
        ),
        // A comma inside the negated clause.
        (
            "trash the drafts in notes/old, but don't touch the finals, notes/q3 or notes/q4",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old but don't, under any circumstances, touch notes/q3",
            "notes/q3",
        ),
        // A mark with no space after it still ends a clause (#1606's review).
        ("trash the drafts,don't touch notes/q3", "notes/q3"),
        ("trash the drafts;don't touch notes/q3", "notes/q3"),
        ("trash the drafts?don't touch notes/q3", "notes/q3"),
    ] {
        assert_eq!(
            crate::goalscope::scope(line, |_| Ok(())),
            Err(kept_off_refusal(path)),
            "{line}"
        );
    }
    for line in [
        "don't touch the finals but trash the drafts in notes/old",
        "don't touch the finals, trash the drafts in notes/old",
        "move everything except the pdfs into notes/archive",
        "trash the drafts in notes/old and leave the rest",
        "sort notes/q3, nothing is urgent",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_ok(), "{line}: {scope:?}");
    }
}

/// #1606's review: a list after a comma whose first item is not a path, the
/// phrasings it named as missed, a child folder by its bare name and the three
/// apostrophe marks consent folds all refuse, naming what is kept off.
#[test]
fn a_list_a_missed_phrasing_and_a_bare_child_folder_are_kept_off() {
    for (line, path) in [
        // A list word right before a path, spaced and glued.
        (
            "trash the drafts in notes/old, but don't touch the finals, the drafts or notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old, but don't touch the finals,drafts or notes/q3",
            "notes/q3",
        ),
        // Missed phrasings.
        (
            "trash the drafts in notes/, but leave out notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/ and leave notes/q3 out",
            "notes/q3",
        ),
        (
            "leave notes/q3 be and trash the drafts in notes/old",
            "notes/q3",
        ),
        ("sort notes/ without touching notes/q3", "notes/q3"),
        (
            "trash the drafts in notes/old; notes/q3 isn't to be touched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; notes/q3 aren't to be moved",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; notes/q3 wasn't to change",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; notes/q3 shan't be touched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; notes/q3 arent to be moved",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; notes/q3 wasnt to change",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; notes/q3 shant be touched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old but neither notes/q3 nor notes/q4",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old but don't touch the finals in notes/q3",
            "notes/q3",
        ),
        // A child folder by its bare name. ("don't touch the q3 folder" is
        // refused before this check: "folder" is a row's cue, negated.)
        ("trash the drafts in notes/ except my q3 folder", "q3"),
        ("trash the drafts in notes/ but skip the q3 folder", "q3"),
        (
            "trash the drafts in notes/ but leave the q3 folder alone",
            "q3",
        ),
        // The marks consent folds as an apostrophe.
        (
            "trash the drafts in notes/old, but don\u{ff07}t touch notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old, but don\u{02b9}t touch notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old, but don\u{055a}t touch notes/q3",
            "notes/q3",
        ),
    ] {
        assert_eq!(
            crate::goalscope::scope(line, |_| Ok(())),
            Err(kept_off_refusal(path)),
            "{line}"
        );
    }
}

/// A negation about something else keeps its goal, as on main.
#[test]
fn a_negation_that_keeps_nothing_off_keeps_the_goal() {
    for line in [
        "sort notes/q3 and never mind the dates",
        "don't worry, just sort notes/q3",
        "trash the drafts in notes/ except the new folder",
        "don't touch the finals, trash the drafts or notes/old",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_ok(), "{line}: {scope:?}");
    }
}

/// #1650's reviews (rounds 77, 77b and 77c): this check only adds to what
/// main refused. Every line from those rounds that main refuses is still
/// refused, naming the folder main names (each checked against main's reader
/// alone),
/// the false refusals among them too ("notes/q3: don't worry about it, just
/// sort it", "move the files that are not pdfs into notes/other", "leave a
/// note in notes/q3 to be read later"): a reworded line costs less than a
/// kept-off folder in the goal.
#[test]
fn every_line_main_refuses_is_still_refused() {
    for (line, path) in MAIN_REFUSES {
        assert_eq!(
            crate::goalscope::scope(line, |_| Ok(())),
            Err(kept_off_refusal(path)),
            "{line}"
        );
    }
    // Main's own miss, which the carry from the last negation reads.
    let line = "the backup is not done so please don't, under any circumstances, touch notes/q3";
    assert_eq!(
        crate::goalscope::scope(line, |_| Ok(())),
        Err(kept_off_refusal("notes/q3"))
    );
}

/// The lines of [`every_line_main_refuses_is_still_refused`].
const MAIN_REFUSES: &[(&str, &str)] = &[
    // Round 77: a negation of worry with a path or a pointer after it.
    ("don't bother with notes/q3", "notes/q3"),
    ("never mind notes/q3", "notes/q3"),
    ("don't worry about notes/q3", "notes/q3"),
    ("don't bother touching notes/q3", "notes/q3"),
    ("notes/q3: don't worry about it", "notes/q3"),
    ("notes/q3: don't worry about it, leave it", "notes/q3"),
    ("notes/q3: don't worry about it, just sort it", "notes/q3"),
    // Round 77: a "not" before "to".
    (
        "trash the drafts in notes/old; you are not to touch notes/q3",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old; it is not ok to touch notes/q3",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old; kai is not allowed into notes/q3",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old; the files that are not to be touched are in notes/q3",
        "notes/q3",
    ),
    (
        "move the files that are not pdfs into notes/other",
        "notes/other",
    ),
    // Round 77: "be" or "as" anywhere after the path.
    (
        "trash the drafts in notes/old but leave notes/q3 just as it is",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old but leave notes/q3 exactly as it is",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old but leave notes/q3 and its files as they are",
        "notes/q3",
    ),
    ("leave a note in notes/q3 to be read later", "notes/q3"),
    // Round 77b, 1: a pointer in a later clause.
    (
        "trash the drafts in notes/. notes/q3: don't bother with it, let it be",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/. notes/q3: don't worry about it, it is done",
        "notes/q3",
    ),
    ("notes/q3: never mind it, keep away from it", "notes/q3"),
    (
        "notes/q3: never mind it. trash the drafts in notes/ that are older than a week",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old, but don't bother with those finals, notes/q3 or notes/q4; just sort them by topic",
        "notes/q3",
    ),
    // Round 77b, 2: a negation of worry with nothing after it.
    ("sort notes/, notes/q3 don't bother", "notes/q3"),
    ("sort notes/, notes/q3 dont bother", "notes/q3"),
    ("sort notes/, notes/q3 don\u{2019}t bother", "notes/q3"),
    (
        "trash the drafts in notes/old, but notes/q3 and notes/q4 never mind",
        "notes/q4",
    ),
    (
        "trash the drafts in notes/old, but don't bother with the finals, notes/q3 or notes/q4",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old but don't worry about the finals, or notes/q3",
        "notes/q3",
    ),
    ("don't worry, notes/q3 is backed up, sort it", "notes/q3"),
    // Round 77b, 3: a describing "not" before an infinitive "to".
    (
        "trash the drafts in notes/old; the files that are not allowed to be touched are in notes/q3",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/, but the files that are not meant to be touched are in notes/q3",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/; anything that is not yours to touch is in notes/q3",
        "notes/q3",
    ),
    (
        "sort notes/ by topic, as a helper who is not allowed to touch notes/q3",
        "notes/q3",
    ),
    // Round 77b, 4: "be" right after "to" ends "leave".
    (
        "trash the drafts in notes/ but leave notes/q3 to be sorted later",
        "notes/q3",
    ),
    (
        "please leave notes/q3 and notes/q4 to be reviewed by me, and trash the drafts in notes/",
        "notes/q3",
    ),
    (
        "leave the drafts in notes/q3 to be checked tomorrow; trash the rest of notes/",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/ but leave my notes/q3 folder to be done by hand",
        "notes/q3",
    ),
    // Round 77b, 5: a new negation before main's Open carry.
    (
        "the backup isn't done so please don't, under any circumstances, touch notes/q3",
        "notes/q3",
    ),
    (
        "the backup isnt done so please don't, under any circumstances, touch notes/q3",
        "notes/q3",
    ),
    (
        "the backups aren't done so please don't, under any circumstances, touch notes/q3",
        "notes/q3",
    ),
    (
        "the backup wasn't done so please don't, under any circumstances, touch notes/q3",
        "notes/q3",
    ),
    (
        "neither of us has a copy so never, ever, touch notes/q3",
        "notes/q3",
    ),
    (
        "we shan't need it so don't, under any circumstances, touch notes/q3",
        "notes/q3",
    ),
    // Round 77c: a new negation or fold right after "and".
    (
        "trash the drafts in notes/old. notes/q3: don't touch the finals and neither the drafts in it",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old but leave notes/q3 and neither of its subfolders should be touched",
        "notes/q3",
    ),
    (
        "notes/q3: don't touch the finals and isn't it done",
        "notes/q3",
    ),
    (
        "notes/q3: don't touch the finals and isnt it done",
        "notes/q3",
    ),
    (
        "notes/q3: don't touch the finals and aren't the drafts in it done",
        "notes/q3",
    ),
    (
        "notes/q3: don't touch the finals and arent the drafts in it done",
        "notes/q3",
    ),
    (
        "notes/q3: don't touch the finals and wasn't it done",
        "notes/q3",
    ),
    (
        "notes/q3: don't touch the finals and wasnt it done",
        "notes/q3",
    ),
    (
        "notes/q3: don't touch the finals and shan't we keep it",
        "notes/q3",
    ),
    (
        "notes/q3: don't touch the finals and shant we keep it",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old. notes/q3: never mind the finals and don\u{ff07}t touch it",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old. notes/q3: never mind the finals and don\u{02b9}t touch it",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old. notes/q3: never mind the finals and don\u{055a}t touch it",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old. notes/q3: don't touch the finals \u{ff07} and neither the drafts in it",
        "notes/q3",
    ),
];

/// The pass that reads as main did is main's text, byte for byte: an edit to
/// it could let a line main refused through (#1650's review).
#[test]
fn the_main_pass_is_mains_reader_unchanged() {
    use sha2::{Digest, Sha256};
    let text = include_str!("../goalkeep/main_pass.rs");
    let sum: String = Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| alloc::format!("{b:02x}"))
        .collect();
    assert_eq!(sum, crate::goalkeep::MAIN_PASS_SHA256);
}

/// "don't touch the q3 folder" is refused before goalkeep reads it: "folder"
/// is a row's cue, negated. The doc relies on that.
#[test]
fn a_negated_folder_is_refused_by_the_rows() {
    assert_eq!(
        crate::goalscope::scope(
            "trash the drafts in notes/ but don't touch the q3 folder",
            |_| Ok(())
        ),
        Err(String::from(
            "\"folder\" is said not to happen; say only what the goal does"
        ))
    );
}

/// #1650's LOWs: a negation #1650 added that only says how a folder is
/// ("isn't sorted yet", "neither ... is sorted yet") keeps the goal, as on
/// main; the same words that keep a folder off still refuse.
#[test]
fn a_new_negation_that_says_how_a_folder_is_keeps_the_goal() {
    for line in [
        "notes/q3 isn't sorted yet, sort it by topic",
        "neither notes/q3 nor notes/q4 is sorted yet, so sort both",
        "notes/q3 isnt done yet, sort it",
        "notes/q3 and notes/q4 aren't sorted, sort them",
        "notes/q3 wasn't backed up, so sort it first",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_ok(), "{line}: {scope:?}");
    }
    for (line, path) in [
        (
            "trash the drafts in notes/old; notes/q3 isn't to be touched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; notes/q3 isn't touched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; neither notes/q3 nor notes/q4 is to be touched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old; neither notes/q3 nor notes/q4 should be touched",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old, notes/q3 isn't sorted, but leave notes/q3 alone",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old but notes/q3 isn't, ever",
            "notes/q3",
        ),
        // A state word with more after it is no state, and a later word that
        // keeps off refuses the path (#1683's review).
        ("notes/q3 isn't ready to be touched", "notes/q3"),
        ("notes/q3 isn't done, hands off", "notes/q3"),
        ("notes/q3 isn't sorted yet, leave it alone", "notes/q3"),
        ("notes/q3 isn't sorted yet, so keep out", "notes/q3"),
        ("notes/q3 isn't sorted yet, so skip it", "notes/q3"),
        (
            "neither notes/q3 nor notes/q4 is ready to be looked at",
            "notes/q3",
        ),
        ("neither notes/q3 nor notes/q4 is done, don't", "notes/q3"),
        ("notes/q3 wasn't backed up so it stays as it is", "notes/q3"),
        ("notes/q3 isn't sorted yet, nobody touches it", "notes/q3"),
        ("notes/q3 isn't sorted yet, so wait", "notes/q3"),
        ("hands off, notes/q3 isn't sorted yet", "notes/q3"),
        ("notes/q3 isn't sorted yet, copy nothing", "notes/q3"),
        (
            "notes/q3 isn't sorted yet, sort it but not today",
            "notes/q3",
        ),
        ("notes/q3 isn't sorted yet, sort it later", "notes/q3"),
        ("nobody touches notes/q3 as it isn't sorted", "notes/q3"),
        ("notes/q3 and notes/q4 aren't done, hands off", "notes/q4"),
        // Its path is still what a later "it" points back to.
        ("notes/q3 isn't sorted, don't touch it", "notes/q3"),
        ("notes/q3 isn't sorted yet; never touch it", "notes/q3"),
    ] {
        assert_eq!(
            crate::goalscope::scope(line, |_| Ok(())),
            Err(kept_off_refusal(path)),
            "{line}"
        );
    }
}

/// #1650's LOWs: misses main also had. A row's cue inside a list item does
/// not end the list, an Oxford comma does not either, and a child folder by
/// its bare name is read as a negation's subject.
#[test]
fn a_cue_in_a_list_item_an_oxford_comma_and_a_bare_folder_subject_are_kept_off() {
    for (line, path) in [
        (
            "trash the drafts in notes/old, but don't touch the finals, the new drafts or notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old, but don't touch the finals, the drafts, or notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/old, but don't touch the finals, the drafts, the new drafts, or notes/q3",
            "notes/q3",
        ),
        (
            "trash the drafts in notes/; the q3 folder isn't to be touched",
            "q3",
        ),
        (
            "trash the drafts in notes/; the q3 folder is not to be touched",
            "q3",
        ),
    ] {
        assert_eq!(
            crate::goalscope::scope(line, |_| Ok(())),
            Err(kept_off_refusal(path)),
            "{line}"
        );
    }
    // An order after the list still ends it.
    for line in [
        "don't touch the finals, trash the drafts or notes/old",
        "don't touch the finals, just trash the drafts or notes/old",
        "don't touch the finals, the drafts, trash notes/old",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_ok(), "{line}: {scope:?}");
    }
}
