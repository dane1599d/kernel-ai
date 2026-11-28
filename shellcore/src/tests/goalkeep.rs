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
