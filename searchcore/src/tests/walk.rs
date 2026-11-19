//! The walk, the index file across a crash, the model's lease, and the caller's
//! `Dir`.

use super::*;
use crate::{BATCH_FILES, IDLE_NS, Opened, Walked};
use alloc::format;
use alloc::vec;
use semindex::Refused;

const MINUTE: u64 = 60 * 1_000_000_000;

fn trip() -> Home {
    let mut home = Home::default();
    home.write("/home/user/notes/trip.txt", "the hotel in lisbon is booked");
    home.write("/home/user/notes/soup.txt", "a soup recipe with bread");
    home
}

#[test]
fn a_file_changes_reports_is_found_after_one_walk() {
    let (mut search, opened) = open(trip(), Disk::default(), Model::keyword());
    assert_eq!(opened, Opened::New);
    let walked = search.walk(0).unwrap();
    assert_eq!(
        walked,
        Walked::Indexed {
            files: 2,
            removed: 0,
            skipped: 0,
            generation: 2
        }
    );
    let reply = search.query(&ask("lisbon hotel"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/trip.txt"]);
    // A walk with nothing new writes nothing.
    let appends = search.store().appends;
    search.walk(0).unwrap();
    assert_eq!(search.store().appends, appends);
}

#[test]
fn the_index_is_read_back_from_its_file_as_it_was() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    let disk = search.store().clone();
    let index = search.index().clone();
    let (again, opened) = open(trip(), disk, Model::keyword());
    assert_eq!(opened, Opened::Read { torn: false });
    assert_eq!(again.index(), &index);
}

/// A crash in the middle of the walk's one batch: the torn tail is cut off when the
/// file is read back, and the next walk indexes the file again.
/// The power goes while a walk appends its batch, and the agent starts again from
/// what is on the disk.
fn crash(search: &mut Service, nth_append: usize) -> Service {
    let at = search.store().appends + nth_append;
    search.store_mut().tear = Some((at, 40));
    assert!(search.walk(0).is_err(), "the power went");
    let disk = Disk {
        tear: None,
        ..search.store().clone()
    };
    let home = core::mem::take(search.files_mut());
    let (search, opened) = open(home, disk, Model::keyword());
    assert_eq!(opened, Opened::Read { torn: true });
    search
}

/// A crash in the middle of the walk's one batch: the torn tail is cut off when the
/// file is read back, and the next walk indexes the file again.
#[test]
fn a_batch_torn_before_its_commit_is_indexed_again() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    search
        .files_mut()
        .write("/home/user/notes/flight.txt", "the flight to lisbon");
    let mut search = crash(&mut search, 1);
    assert_eq!(
        search.index().generation,
        2,
        "the mark is where the walk began"
    );
    assert!(
        search
            .index()
            .files()
            .all(|f| !f.path.ends_with("flight.txt"))
    );
    assert_eq!(search.query(&ask("flight"), 0, &mut everything()), "ok 0");
    search.walk(0).unwrap();
    assert_eq!(search.index().generation, 3);
    let reply = search.query(&ask("flight"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/flight.txt"]);
}

/// A crash in a later batch of a long walk: the batches before it are kept, the
/// mark stays where the walk began, and the next walk does them all again.
#[test]
fn a_walk_cut_short_between_batches_is_done_again_from_its_start() {
    let mut home = Home::default();
    for n in 0..BATCH_FILES + 4 {
        let text = format!("invoice number {n}");
        home.write(&format!("/home/user/inbox/{n:02}.txt"), &text);
    }
    let (mut search, _) = open(home, Disk::default(), Model::keyword());
    let mut search = crash(&mut search, 2);
    assert_eq!(search.index().files().count(), BATCH_FILES);
    assert_eq!(
        search.index().generation,
        0,
        "only the last batch moves the mark"
    );
    search.walk(0).unwrap();
    assert_eq!(search.index().files().count(), BATCH_FILES + 4);
    assert_eq!(search.index().generation, (BATCH_FILES + 4) as u64);
}

#[test]
fn an_index_of_another_format_or_embedder_is_rebuilt_not_read() {
    let (mut keyword, _) = open(trip(), Disk::default(), Model::keyword());
    keyword.walk(0).unwrap();
    let words_only = keyword.store().clone();
    // The same file, opened for an index whose vectors come from a model.
    let (mut search, opened) = open(trip(), words_only, Model::topics());
    assert_eq!(opened, Opened::Rebuilt(Refused::Embedder));
    assert_eq!(search.index().files().count(), 0, "nothing of it was read");
    assert_eq!(search.index().generation, 0);
    search.walk(0).unwrap();
    assert_eq!(search.index().files().count(), 2);
    // Another format, or bytes that are no index at all.
    for bytes in [b"kai-index 0\n".to_vec(), b"\x00\x01garbage".to_vec()] {
        let disk = Disk {
            bytes,
            ..Disk::default()
        };
        let (search, opened) = open(trip(), disk, Model::keyword());
        assert_eq!(opened, Opened::Rebuilt(Refused::Magic));
        assert_eq!(
            search.store().bytes,
            semindex::fresh(&Default::default()).unwrap()
        );
    }
}

#[test]
fn indexing_waits_while_there_is_no_room_for_the_model() {
    let model = Model {
        room: false,
        ..Model::topics()
    };
    let loads = model.loads.clone();
    let (mut search, _) = open(trip(), Disk::default(), model);
    let appends = search.store().appends;
    assert_eq!(search.walk(0).unwrap(), Walked::NoRoom);
    assert_eq!(loads.get(), 0, "never mapped");
    assert_eq!(search.store().appends, appends, "nothing written");
    assert_eq!(search.index().files().count(), 0);
    // A query still answers, by keyword, with nothing mapped.
    let reply = search.query(&ask("lisbon"), 0, &mut everything());
    assert_eq!(paths(&reply), Vec::<String>::new());
    assert_eq!(loads.get(), 0);
    search.models_mut().room = true;
    assert!(matches!(
        search.walk(0).unwrap(),
        Walked::Indexed { files: 2, .. }
    ));
    assert_eq!(loads.get(), 1);
}

#[test]
fn the_model_is_unmapped_after_five_idle_minutes() {
    let model = Model::topics();
    let (loads, live) = (model.loads.clone(), model.live.clone());
    let (mut search, _) = open(trip(), Disk::default(), model);
    search.walk(0).unwrap();
    assert!(search.holds_model());
    assert_eq!(live.get(), 1);
    assert!(!search.idle(5 * MINUTE - 1), "not yet");
    // A query uses it, which starts the five minutes again.
    let reply = search.query(&ask("travel"), 4 * MINUTE, &mut everything());
    assert_eq!(
        paths(&reply),
        ["/home/user/notes/trip.txt"],
        "found by meaning"
    );
    assert!(!search.idle(4 * MINUTE + IDLE_NS - 1));
    assert!(search.idle(4 * MINUTE + IDLE_NS));
    assert!(!search.holds_model());
    assert_eq!(live.get(), 0, "unmapped");
    // The next use maps it again.
    search.query(&ask("travel"), 20 * MINUTE, &mut everything());
    assert_eq!((loads.get(), live.get()), (2, 1));
}

/// A hit whose path the caller's `Dir` refuses is never returned, and does not take
/// a place under the limit.
#[test]
fn a_hit_the_callers_dir_refuses_is_never_returned() {
    let mut home = trip();
    home.write(
        "/home/user/private/lisbon.txt",
        "lisbon lisbon lisbon hotel hotel",
    );
    let (mut search, _) = open(home, Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    let all = search.query(&ask("lisbon hotel"), 0, &mut everything());
    assert_eq!(
        paths(&all)[0],
        "/home/user/private/lisbon.txt",
        "the best match"
    );
    let mut notes = Dir(vec!["/home/user/notes"]);
    let request = crate::text::Request {
        limit: Some(1),
        ..crate::text::Request::parse(&ask("lisbon hotel")).unwrap()
    };
    let reply = search.query(&request.encode(), 0, &mut notes);
    assert_eq!(paths(&reply), ["/home/user/notes/trip.txt"]);
    assert!(!reply.contains("private"), "{reply}");
    let mut nothing = Dir(vec![]);
    assert_eq!(search.query(&ask("lisbon"), 0, &mut nothing), "ok 0");
}

#[test]
fn a_file_deleted_renamed_or_replaced_follows_in_the_index() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    search.walk(0).unwrap();
    let home = search.files_mut();
    home.delete("/home/user/notes/soup.txt");
    home.rename("/home/user/notes/trip.txt", "/home/user/travel/trip.txt");
    // A new file where the old trip was: another object, at its old path.
    home.write("/home/user/notes/trip.txt", "the agenda for the meeting");
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(
            walked,
            Walked::Indexed {
                files: 2,
                removed: 1,
                ..
            }
        ),
        "{walked:?}"
    );
    let reply = search.query(&ask("lisbon"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/travel/trip.txt"]);
    let reply = search.query(&ask("agenda"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/notes/trip.txt"]);
    assert_eq!(search.query(&ask("soup"), 0, &mut everything()), "ok 0");
    // What it holds reads back from its file.
    let disk = search.store().clone();
    let index = search.index().clone();
    let (again, _) = open(Home::default(), disk, Model::keyword());
    assert_eq!(again.index(), &index);
}

#[test]
fn files_outside_the_scope_too_large_or_not_text_are_skipped() {
    let mut home = Home::default();
    home.write("/home/user/a.txt", "lisbon");
    home.write("/home/user/b.bin", "\0\u{1}binary");
    home.write(
        "/home/user/big.txt",
        &"x ".repeat((semindex::index::MAX_FILE / 2 + 1) as usize),
    );
    let mut scope = Scope::new();
    scope.exclude("/home/user/secret").unwrap();
    home.write("/home/user/secret/lisbon.txt", "lisbon");
    let (mut search, _) = Search::open(home, Disk::default(), Model::keyword(), scope).unwrap();
    let walked = search.walk(0).unwrap();
    assert!(
        matches!(
            walked,
            Walked::Indexed {
                files: 1,
                skipped: 3,
                ..
            }
        ),
        "{walked:?}"
    );
    let reply = search.query(&ask("lisbon"), 0, &mut everything());
    assert_eq!(paths(&reply), ["/home/user/a.txt"]);
}

#[test]
fn a_request_out_of_shape_is_answered_with_an_err() {
    let (mut search, _) = open(trip(), Disk::default(), Model::keyword());
    for request in ["", "search 2\nquery x", "search 1\nquery ", "query lisbon"] {
        let reply = search.query(request, 0, &mut everything());
        assert_eq!(reply, "err not a search 1 request");
    }
}
