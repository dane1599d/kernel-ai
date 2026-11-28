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

/// #1606's review: a line that keeps nothing off is not refused: a "not" that
/// describes files, "be" later in a "leave" clause, and a negation of worry.
#[test]
fn a_negation_that_keeps_nothing_off_keeps_the_goal() {
    for line in [
        "move the files that are not pdfs into notes/other",
        "move the files that aren't pdfs into notes/other",
        "leave a note in notes/q3 to be read later",
        "notes/q3: don't worry about it, just sort it",
        "sort notes/q3 and never mind the dates",
        "trash the drafts in notes/ except the new folder",
        "don't touch the finals, trash the drafts or notes/old",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_ok(), "{line}: {scope:?}");
    }
}
