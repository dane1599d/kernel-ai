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

/// #1650's reviews (rounds 77 and 77b): this check only adds to what main
/// refused. Every line from both rounds that main refuses is still refused,
/// the false refusals among them too ("notes/q3: don't worry about it, just
/// sort it", "move the files that are not pdfs into notes/other", "leave a
/// note in notes/q3 to be read later"): a reworded line costs less than a
/// kept-off folder in the goal.
#[test]
fn every_line_main_refuses_is_still_refused() {
    for line in MAIN_REFUSES {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_err(), "{line}: {scope:?}");
    }
    // Main's own miss, which the carry from the last negation reads.
    let line = "the backup is not done so please don't, under any circumstances, touch notes/q3";
    assert_eq!(
        crate::goalscope::scope(line, |_| Ok(())),
        Err(kept_off_refusal("notes/q3"))
    );
}

/// The lines of [`every_line_main_refuses_is_still_refused`].
const MAIN_REFUSES: &[&str] = &[
    // Round 77: a negation of worry with a path or a pointer after it.
    "don't bother with notes/q3",
    "never mind notes/q3",
    "don't worry about notes/q3",
    "don't bother touching notes/q3",
    "notes/q3: don't worry about it",
    "notes/q3: don't worry about it, leave it",
    "notes/q3: don't worry about it, just sort it",
    // Round 77: a "not" before "to".
    "trash the drafts in notes/old; you are not to touch notes/q3",
    "trash the drafts in notes/old; it is not ok to touch notes/q3",
    "trash the drafts in notes/old; kai is not allowed into notes/q3",
    "trash the drafts in notes/old; the files that are not to be touched are in notes/q3",
    "move the files that are not pdfs into notes/other",
    // Round 77: "be" or "as" anywhere after the path.
    "trash the drafts in notes/old but leave notes/q3 just as it is",
    "trash the drafts in notes/old but leave notes/q3 exactly as it is",
    "trash the drafts in notes/old but leave notes/q3 and its files as they are",
    "leave a note in notes/q3 to be read later",
    // Round 77b, 1: a pointer in a later clause.
    "trash the drafts in notes/. notes/q3: don't bother with it, let it be",
    "trash the drafts in notes/. notes/q3: don't worry about it, it is done",
    "notes/q3: never mind it, keep away from it",
    "notes/q3: never mind it. trash the drafts in notes/ that are older than a week",
    "trash the drafts in notes/old, but don't bother with those finals, notes/q3 or notes/q4; just sort them by topic",
    // Round 77b, 2: a negation of worry with nothing after it.
    "sort notes/, notes/q3 don't bother",
    "sort notes/, notes/q3 dont bother",
    "sort notes/, notes/q3 don\u{2019}t bother",
    "trash the drafts in notes/old, but notes/q3 and notes/q4 never mind",
    "trash the drafts in notes/old, but don't bother with the finals, notes/q3 or notes/q4",
    "trash the drafts in notes/old but don't worry about the finals, or notes/q3",
    "don't worry, notes/q3 is backed up, sort it",
    // Round 77b, 3: a describing "not" before an infinitive "to".
    "trash the drafts in notes/old; the files that are not allowed to be touched are in notes/q3",
    "trash the drafts in notes/, but the files that are not meant to be touched are in notes/q3",
    "trash the drafts in notes/; anything that is not yours to touch is in notes/q3",
    "sort notes/ by topic, as a helper who is not allowed to touch notes/q3",
    // Round 77b, 4: "be" right after "to" ends "leave".
    "trash the drafts in notes/ but leave notes/q3 to be sorted later",
    "please leave notes/q3 and notes/q4 to be reviewed by me, and trash the drafts in notes/",
    "leave the drafts in notes/q3 to be checked tomorrow; trash the rest of notes/",
    "trash the drafts in notes/ but leave my notes/q3 folder to be done by hand",
    // Round 77b, 5: a new negation before main's Open carry.
    "the backup isn't done so please don't, under any circumstances, touch notes/q3",
    "the backup isnt done so please don't, under any circumstances, touch notes/q3",
    "the backups aren't done so please don't, under any circumstances, touch notes/q3",
    "the backup wasn't done so please don't, under any circumstances, touch notes/q3",
    "neither of us has a copy so never, ever, touch notes/q3",
    "we shan't need it so don't, under any circumstances, touch notes/q3",
];
