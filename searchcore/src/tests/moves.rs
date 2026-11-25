//! Folders moved, trashed or excluded, reads that fail, and the full reconcile:
//! the index follows what is in the home folder, and spends no read on what did
//! not change.

use super::walk::trip;
use super::*;
use crate::Walked;
use alloc::vec::Vec;

/// The review's second HIGH: a folder renamed carries its files to their new
/// paths, re-keyed with the chunks they have, and none is read again.
#[test]
fn a_renamed_folder_re_keys_its_files_without_reading_them() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    let reads = search.files_mut().reads;
    search
        .files_mut()
        .rename_folder("/home/user/notes", "/home/user/archive/2026");
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(walked, Walked::Indexed { failed: 0, .. }),
        "{walked:?}"
    );
    assert_eq!(search.files_mut().reads, reads, "a move reads nothing");
    let reply = search.query(&ask("lisbon"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/archive/2026/trip.txt"]);
    let mut kept: Vec<&str> = search.index().files().map(|f| f.path.as_str()).collect();
    kept.sort();
    assert_eq!(
        kept,
        [
            "/home/user/archive/2026/soup.txt",
            "/home/user/archive/2026/trip.txt"
        ]
    );
    let disk = search.store().clone();
    let index = search.index().clone();
    let (again, _) = open(Home::default(), disk, Model::keyword());
    assert_eq!(again.index(), &index);
}

/// A trashed folder's files leave the index, though no row names them: only the
/// folder it was in changed.
#[test]
fn a_trashed_folder_takes_its_files_out_of_the_index() {
    let mut home = trip();
    home.write("/home/user/travel/plan.txt", "lisbon by train");
    let (mut search, _) = open(home, Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    search.files_mut().trash_folder("/home/user/notes");
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(walked, Walked::Indexed { removed: 2, .. }),
        "{walked:?}"
    );
    let reply = search.query(&ask("lisbon"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/travel/plan.txt"]);
    assert_eq!(search.query(&ask("soup"), 0, &mut everything()), "ok 0");
}

/// A folder moved into one the person excluded takes its files out of the index,
/// and moved back out, they come back.
#[test]
fn a_folder_moved_into_an_excluded_one_leaves_and_comes_back() {
    let mut scope = Scope::new();
    scope.exclude("/home/user/secret").unwrap();
    let (mut search, _) = Search::open(trip(), Disk::default(), Model::keyword(), scope).unwrap();
    search.walk(0).unwrap();
    search
        .files_mut()
        .rename_folder("/home/user/notes", "/home/user/secret/notes");
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(walked, Walked::Indexed { removed: 2, .. }),
        "{walked:?}"
    );
    assert_eq!(search.index().files().count(), 0);
    search
        .files_mut()
        .rename_folder("/home/user/secret/notes", "/home/user/notes");
    search.walk(0).unwrap();
    let reply = search.query(&ask("lisbon"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/trip.txt"]);
}

/// The review's MEDIUM: a file that cannot be read now keeps its old entry, and
/// the mark stays below its change, so the next walk reads it again; a change
/// after it is indexed meanwhile.
#[test]
fn a_read_that_fails_keeps_the_entry_and_is_tried_again() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    let home = search.files_mut();
    home.write("/home/user/notes/trip.txt", "the flight to porto");
    home.write("/home/user/notes/bread.txt", "bread recipe");
    home.fail_reads
        .push(String::from("/home/user/notes/trip.txt"));
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(
            walked,
            Walked::Indexed {
                failed: 1,
                generation: 2,
                ..
            }
        ),
        "{walked:?}"
    );
    // Its old entry is still there, and still found.
    let reply = search.query(&ask("lisbon"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/trip.txt"]);
    let reply = search.query(&ask("bread"), 0, &mut everything());
    assert!(
        paths(&reply)
            .iter()
            .any(|p| p == "/home/user/notes/bread.txt"),
        "{reply}"
    );
    // The next walk reads it again.
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(
            walked,
            Walked::Indexed {
                failed: 0,
                generation: 4,
                ..
            }
        ),
        "{walked:?}"
    );
    let reply = search.query(&ask("porto"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/trip.txt"]);
    assert_eq!(search.query(&ask("lisbon"), 0, &mut everything()), "ok 0");
}

/// The fix check's first HIGH: a folder moved with one of its files also changed
/// still carries every other file to its new path.
#[test]
fn a_moved_folder_with_a_changed_file_keeps_all_its_files() {
    let mut home = trip();
    home.write("/home/user/notes/bread.txt", "bread recipe");
    let (mut search, _) = open(home, Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    let home = search.files_mut();
    home.rename_folder("/home/user/notes", "/home/user/archive");
    home.write("/home/user/archive/trip.txt", "the hotel in lisbon is paid");
    search.walk(0).unwrap();
    let mut kept: Vec<&str> = search.index().files().map(|f| f.path.as_str()).collect();
    kept.sort();
    assert_eq!(
        kept,
        [
            "/home/user/archive/bread.txt",
            "/home/user/archive/soup.txt",
            "/home/user/archive/trip.txt"
        ]
    );
}

/// The fix check's second HIGH: a file that never reads is given up after
/// MAX_TRIES walks, and the mark passes it, so nothing is read again for it.
#[test]
fn a_file_that_never_reads_is_given_up_and_the_mark_passes_it() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    let home = search.files_mut();
    home.write("/home/user/notes/trip.txt", "the flight to porto");
    home.write("/home/user/notes/bread.txt", "bread recipe");
    home.fail_always
        .push(String::from("/home/user/notes/trip.txt"));
    for _ in 1..crate::MAX_TRIES {
        let walked = search.walk(0).unwrap();
        assert!(
            matches!(
                walked,
                Walked::Indexed {
                    failed: 1,
                    generation: 2,
                    ..
                }
            ),
            "{walked:?}"
        );
    }
    let walked = search.walk(0).unwrap();
    let Walked::Indexed {
        gave_up,
        generation,
        ..
    } = walked
    else {
        panic!("{walked:?}");
    };
    assert_eq!(gave_up, ["/home/user/notes/trip.txt"]);
    assert_eq!(generation, 4, "the mark passed it");
    assert_eq!(search.query(&ask("lisbon"), 0, &mut everything()), "ok 0");
    let reads = search.files_mut().reads;
    search.walk(0).unwrap();
    assert_eq!(
        search.files_mut().reads,
        reads,
        "nothing is read again for it"
    );
    // A change to it is a new chance.
    let home = search.files_mut();
    home.fail_always.clear();
    home.write("/home/user/notes/trip.txt", "the flight to porto");
    search.walk(0).unwrap();
    let reply = search.query(&ask("porto"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/trip.txt"]);
}

/// The fix check's MEDIUM: a read that fails inside a folder moved in is tried
/// again, though nothing under the folder changed after.
#[test]
fn a_failed_read_in_a_moved_in_folder_is_tried_again() {
    let mut scope = Scope::new();
    scope.exclude("/home/user/secret").unwrap();
    let mut home = trip();
    home.write("/home/user/secret/plans/a.txt", "a plan for lisbon");
    home.write("/home/user/secret/plans/b.txt", "a plan for porto");
    let (mut search, _) = Search::open(home, Disk::default(), Model::keyword(), scope).unwrap();
    search.walk(0).unwrap();
    let home = search.files_mut();
    home.rename_folder("/home/user/secret/plans", "/home/user/plans");
    home.fail_reads.push(String::from("/home/user/plans/b.txt"));
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(walked, Walked::Indexed { failed: 1, .. }),
        "{walked:?}"
    );
    search.walk(0).unwrap();
    let reply = search.query(&ask("porto"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/plans/b.txt"]);
}

/// A change `fs` may have missed is found by the full reconcile: when its fallback
/// count moves, and on the first walk after every start. Nothing unchanged is read.
#[test]
fn a_fallback_or_a_start_reconciles_the_whole_home_folder() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    search
        .files_mut()
        .delete_unseen("/home/user/notes/soup.txt");
    search.walk(0).unwrap();
    assert_eq!(search.index().files().count(), 2, "no row said so");
    let reads = search.files_mut().reads;
    search.files_mut().fallbacks += 1;
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(walked, Walked::Indexed { removed: 1, .. }),
        "{walked:?}"
    );
    assert_eq!(search.files_mut().reads, reads, "nothing unchanged is read");
    // A start: the agent restarts over a home folder changed while it was down.
    search
        .files_mut()
        .delete_unseen("/home/user/notes/trip.txt");
    let disk = search.store().clone();
    let home = core::mem::take(search.files_mut());
    let (mut again, _) = open(home, disk, Model::keyword());
    again.walk(0).unwrap();
    assert_eq!(again.index().files().count(), 0);
}

/// A fall back mount undoes commits already indexed: the index takes back what
/// was undone, its mark follows `fs`'s back, and a change made after the fall back,
/// at a generation the old mark had passed, is still indexed.
#[test]
fn a_fall_back_that_undoes_commits_takes_the_index_back_with_it() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    let earlier = search.files_mut().clone();
    let home = search.files_mut();
    home.write("/home/user/notes/trip.txt", "the flight to porto");
    home.write("/home/user/notes/bread.txt", "bread recipe");
    search.walk(0).unwrap();
    assert_eq!(search.index().generation, 4);
    search.files_mut().fall_back_to(&earlier);
    search.walk(0).unwrap();
    assert_eq!(search.index().generation, 2, "the mark went back with fs");
    let reply = search.query(&ask("lisbon"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/trip.txt"], "undone back");
    assert_eq!(search.query(&ask("porto"), 0, &mut everything()), "ok 0");
    let reply = search.query(&ask("bread"), 0, &mut everything());
    assert!(!reply.contains("bread.txt"), "{reply}");
    // A change after the fall back, at generation 3, below the old mark of 4.
    search
        .files_mut()
        .write("/home/user/notes/tea.txt", "green tea");
    search.walk(0).unwrap();
    let reply = search.query(&ask("tea"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/tea.txt"]);
    // The index file holds it all, and reads back as it is.
    let disk = search.store().clone();
    let index = search.index().clone();
    let (again, opened) = open(Home::default(), disk, Model::keyword());
    assert_eq!(opened, crate::Opened::Read { torn: false });
    assert_eq!(again.index(), &index);
}
