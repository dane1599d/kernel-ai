//! #1684's review hunt for the state-word release (`crate::goalkeep`): lines
//! that keep a folder off, each refused and named as current main names it.

/// #1684's review hunt: 80 lines that keep a folder off which the release
/// read at ec550fbe let through and current main refuses. Each still
/// refuses, naming its folder.
const HUNT_1684: &[(&str, &str)] = &[
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
    ("notes/q3 isn't sorted, don't touch it", "notes/q3"),
    ("notes/q3 isn't sorted yet; never touch it", "notes/q3"),
    (
        "trash the drafts in notes/, notes/q3 isn't sorted yet so don't touch it",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old; notes/q3 isn't done and is not to be touched",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/; notes/q3: isn't ready so leave it alone",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/, but notes/q3 isn't sorted yet, so leave it alone",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old; notes/q3 isn't done and isn't to be touched",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old; notes/q3 isn't done but must not be touched",
        "notes/q3",
    ),
    ("notes/q3 wasn't backed up, so keep out of it", "notes/q3"),
    (
        "trash the drafts in notes/; notes/q3 and notes/q4 aren't sorted, don't touch them",
        "notes/q4",
    ),
    (
        "trash the drafts in notes/; neither notes/q3 nor notes/q4 is ready, so don't touch them",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/old; notes/q3 isn't ready to be touched",
        "notes/q3",
    ),
    (
        "trash the drafts in notes/; neither notes/q3 nor anything that is done should be touched",
        "notes/q3",
    ),
    (
        "trash everything in notes/ other than the ones that aren't sorted, notes/q3 and notes/q4",
        "notes/q3",
    ),
    ("notes/q3 isn't backed up so avoid it", "notes/q3"),
    (
        "notes/q3 isn't done being reviewed, so leave it",
        "notes/q3",
    ),
    (
        "neither notes/q3 nor notes/q4 is done yet and neither is to be touched",
        "notes/q3",
    ),
    ("the q3 folder isn't to be touched", "q3"),
    (
        "trash the drafts in notes/, the q3 folder isn't sorted, don't touch it",
        "q3",
    ),
    (
        "trash the drafts in notes/ but notes/q3 isn't ready to be touched",
        "notes/q3",
    ),
    (
        "sort notes/; notes/q3 isn't done being reviewed, so leave it",
        "notes/q3",
    ),
    (
        "clean up notes/, notes/q3 isn't backed up yet so stay out of it",
        "notes/q3",
    ),
    (
        "notes/q3 isn't sorted and must stay that way; tidy the rest of notes/",
        "notes/q3",
    ),
    (
        "neither notes/q3 nor notes/q4 is ready for changes; sort notes/",
        "notes/q3",
    ),
    (
        "neither notes/q3 nor notes/q4 is ready to be touched; sort notes/",
        "notes/q3",
    ),
    ("notes/q3 wasn't finished, keep away", "notes/q3"),
    ("notes/q3 isn't done, hands off", "notes/q3"),
    ("notes/q3 isn't safe to move; reorganise notes/", "notes/q3"),
    ("notes/q3 isn't final yet - do not modify", "notes/q3"),
    ("notes/q3 isn't done yet - do not modify", "notes/q3"),
    ("notes/q3 isn't sorted so don't touch it", "notes/q3"),
    ("notes/q3 isn't ready - don't touch it", "notes/q3"),
    ("NOTES/Q3 ISN'T READY - DON'T TOUCH", "notes/q3"),
    ("notes/q3 isn't ready, so it can't be touched", "notes/q3"),
    ("notes/q3 isn't ready, it shouldn't be touched", "notes/q3"),
    ("notes/q3 isn't ready so do not touch it", "notes/q3"),
    (
        "trash the drafts in notes/ but notes/q3 isn't done yet so the goal can't touch it",
        "notes/q3",
    ),
    (
        "notes/q3 and notes/q4 aren't sorted yet, don't touch them",
        "notes/q4",
    ),
    (
        "neither notes/q3 nor notes/q4 is ready, don't touch them",
        "notes/q3",
    ),
    (
        "neither notes/q3 nor notes/q4 is done yet, so keep out of both",
        "notes/q3",
    ),
    (
        "neither notes/q3 nor notes/q4 is ready, leave them alone",
        "notes/q3",
    ),
    (
        "neither notes/q3 nor notes/q4 is done, so neither should be touched",
        "notes/q3",
    ),
    (
        "neither of notes/q3 or notes/q4 is done, so don't touch either",
        "notes/q3",
    ),
    (
        "neither notes/q3 nor the drafts that are done should be moved",
        "notes/q3",
    ),
    ("notes/q3 isn't sorted, leave it alone", "notes/q3"),
    ("notes/q3 isn't done yet so leave it be", "notes/q3"),
    ("notes/q3 isn't done, leave it out", "notes/q3"),
    ("notes/q3 wasn't backed up so don't risk it", "notes/q3"),
    (
        "as for notes/q3, it isn't done so it can't be touched",
        "notes/q3",
    ),
    (
        "notes/q3 isn't backed up, nothing in it should be touched",
        "notes/q3",
    ),
    (
        "notes/q3 isn't organised yet and I want it kept that way; tidy notes/",
        "notes/q3",
    ),
    (
        "notes/q3 isn't tagged yet, the goal should stay out of it",
        "notes/q3",
    ),
    (
        "notes/q3 isn't synced; until it is, keep the goal away from it",
        "notes/q3",
    ),
    ("notes/q3 isn't clean yet so hands off for now", "notes/q3"),
    (
        "notes/q3 isn't ready for the goal; sort the rest of notes/",
        "notes/q3",
    ),
    (
        "notes/q3 isn't ready to be sorted yet; sort the rest of notes/",
        "notes/q3",
    ),
    ("notes/q3 isn't ready, so it's off limits", "notes/q3"),
    ("notes/q3 isn't done - it's off limits", "notes/q3"),
    ("notes/q3 isn't finished so it stays untouched", "notes/q3"),
    (
        "notes/q3 isn’t ready to be touched; sort notes/",
        "notes/q3",
    ),
    (
        "notes/q3 isn＇t ready to be touched; sort notes/",
        "notes/q3",
    ),
    ("notes/q3 isnt done, hands off", "notes/q3"),
    ("Notes/Q3 Wasnt Finished, Keep Away", "notes/q3"),
    (
        "notes/q3 and notes/q4 aren't all done, keep out",
        "notes/q4",
    ),
    ("sort notes/ but keep away, notes/q3 isn't done", "notes/q3"),
    (
        "the goal isn't ready for notes/q3, so leave it out",
        "notes/q3",
    ),
    ("the drafts aren't done in my q3 folder, so stay out", "q3"),
    (
        "my finals aren't done, notes/q3 included, so leave them",
        "notes/q3",
    ),
    (
        "the reports aren't finished, notes/q3 and notes/q4 especially, so keep out",
        "notes/q3",
    ),
    (
        "notes/q3 isn't done being edited, skip it for now",
        "notes/q3",
    ),
    (
        "sort notes/ but notes/q3 isn't quite finished, so skip it",
        "notes/q3",
    ),
    ("notes/q3 isn't done yet, avoid it", "notes/q3"),
    (
        "notes/q3 isn't complete; the goal must not go near it",
        "notes/q3",
    ),
    (
        "notes/q3 isn't touched by anything, keep it that way",
        "notes/q3",
    ),
];

#[test]
fn every_hunt_line_of_1684_refuses_its_folder() {
    let mut missed = alloc::vec::Vec::new();
    for (line, path) in HUNT_1684 {
        // Refused, and goalkeep names the folder (some lines are refused
        // first by the goal's rows, as on main: "move", "go", "folder").
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        let named = crate::goalkeep::kept_off(line, crate::goalscope::starts_order);
        if scope.is_ok() || named.as_deref() != Some(*path) {
            missed.push(alloc::format!("{line}: {named:?}, {scope:?}"));
        }
    }
    assert!(
        missed.is_empty(),
        "{} of {}:\n{}",
        missed.len(),
        HUNT_1684.len(),
        missed.join("\n")
    );
}
