//! A download a chunk at a time (`Step`), as `updated` steps one between requests.

use super::*;

/// A download a chunk at a time (`updated` steps one between requests): the
/// same chunks in the same order as `download`, read-backs first, a chunk
/// that went bad on disk fetched again, and done only once.
#[test]
fn a_stepped_download_does_what_download_does_a_chunk_at_a_time() {
    let target = blob(6 * MIN_CHUNK as usize + 11);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Memory {
        bytes: target[..4 * MIN_CHUNK as usize].to_vec(),
        done: 4,
        ..Memory::default()
    };
    partial.bytes[2 * MIN_CHUNK as usize + 3] ^= 0x40;
    let mut server = Server::new(target.clone());
    let mut fetch =
        |range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| server.fetch(range, sink);
    let mut run = Download::start(list.clone(), &mut partial).unwrap();
    let mut steps = Vec::new();
    loop {
        let step = run.step(&mut partial, &mut fetch).unwrap();
        steps.push(step);
        if matches!(step, Step::Done(_)) {
            break;
        }
        assert_eq!(run.progress(), steps.len());
    }
    let report = Report {
        kept: 2,
        fetched: 5,
    };
    assert_eq!(
        steps,
        [
            Step::Checked(0),
            Step::Checked(1),
            Step::Fetched(2),
            Step::Fetched(3),
            Step::Fetched(4),
            Step::Fetched(5),
            Step::Fetched(6),
            Step::Done(report),
        ]
    );
    // The bad chunk's checkpoint went back to 2 before it was fetched again.
    assert_eq!(partial.records, [2, 3, 4, 5, 6, 7]);
    assert_eq!(partial.bytes, target);
    assert!(partial.finished);
    // Done once: asked again, it answers the same and moves nothing in twice.
    partial.finished = false;
    assert_eq!(run.step(&mut partial, &mut fetch), Ok(Step::Done(report)));
    assert!(!partial.finished);
    assert_eq!(run.list(), &list);
}

/// A failed step can be stepped again (`updated` tries the same chunk after a
/// wait): a cut fetch, then the `finish` that failed, with the whole hash
/// still the target's.
#[test]
fn a_step_that_failed_is_stepped_again_from_where_it_was() {
    let target = blob(3 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Memory {
        failed_finishes: 1,
        ..Memory::default()
    };
    let mut server = Server::new(target.clone());
    server.cuts = vec![MIN_CHUNK + 7];
    let mut fetch =
        |range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| server.fetch(range, sink);
    let mut run = Download::start(list, &mut partial).unwrap();
    assert_eq!(run.step(&mut partial, &mut fetch), Ok(Step::Fetched(0)));
    assert_eq!(
        run.step(&mut partial, &mut fetch),
        Err(DownloadError::Fetch(HttpError::Truncated))
    );
    assert_eq!(run.progress(), 1);
    assert_eq!(run.step(&mut partial, &mut fetch), Ok(Step::Fetched(1)));
    assert_eq!(run.step(&mut partial, &mut fetch), Ok(Step::Fetched(2)));
    assert_eq!(
        run.step(&mut partial, &mut fetch),
        Err(DownloadError::Store("the move failed"))
    );
    assert!(!partial.finished);
    let report = Report {
        kept: 0,
        fetched: 3,
    };
    assert_eq!(run.step(&mut partial, &mut fetch), Ok(Step::Done(report)));
    assert!(partial.finished);
    assert_eq!(partial.bytes, target);
}

/// A chunk the checkpoint names that cannot be read back, as a file cut short
/// by a crash leaves it, is fetched again with the chunks after it.
#[test]
fn a_recorded_chunk_that_cannot_be_read_back_is_fetched_again() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Memory {
        bytes: target[..2 * MIN_CHUNK as usize + 5].to_vec(),
        done: 3,
        ..Memory::default()
    };
    let mut server = Server::new(target.clone());
    assert_eq!(
        run(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 2,
            fetched: 2
        })
    );
    assert_eq!(partial.records, [2, 3, 4]);
    assert_eq!(partial.bytes, target);
}

/// A stepped run whose shared partial changed on disk after its check: the
/// step that would move it in refuses, and the next steps start the run again
/// from the mark it set back, chunk 0 read back and chunk 1 on fetched.
#[test]
fn a_stepped_run_starts_again_from_a_chunk_changed_on_disk() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        flip: Some((4, MIN_CHUNK as usize + 9)),
        ..Watched::default()
    };
    let mut server = Server::new(target.clone());
    let mut fetch =
        |range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| server.fetch(range, sink);
    let mut run = Download::start(list, &mut partial).unwrap();
    for i in 0..4 {
        assert_eq!(run.step(&mut partial, &mut fetch), Ok(Step::Fetched(i)));
    }
    assert_eq!(
        run.step(&mut partial, &mut fetch),
        Err(DownloadError::Store(CHANGED_ON_DISK))
    );
    assert_eq!(run.progress(), 0);
    let mut steps = Vec::new();
    loop {
        let step = run.step(&mut partial, &mut fetch).unwrap();
        steps.push(step);
        if matches!(step, Step::Done(_)) {
            break;
        }
    }
    let report = Report {
        kept: 1,
        fetched: 3,
    };
    assert_eq!(
        steps,
        [
            Step::Checked(0),
            Step::Fetched(1),
            Step::Fetched(2),
            Step::Fetched(3),
            Step::Done(report),
        ]
    );
    assert!(partial.inner.finished);
    assert_eq!(partial.inner.bytes, target);
}

/// A read of a recorded chunk that fails other than short, as a store that
/// fails for a moment does, is passed on: the mark stays where it was, and
/// stepping again reads the same chunk back and keeps it and those after it.
#[test]
fn a_failed_read_on_resume_is_passed_on_and_the_mark_kept() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        fail_read: Some((MIN_CHUNK, "a read failed")),
        ..Watched::default()
    };
    partial.inner.bytes = target[..3 * MIN_CHUNK as usize].to_vec();
    partial.inner.done = 3;
    let mut server = Server::new(target.clone());
    let mut fetch =
        |range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| server.fetch(range, sink);
    let mut run = Download::start(list, &mut partial).unwrap();
    assert_eq!(run.step(&mut partial, &mut fetch), Ok(Step::Checked(0)));
    assert_eq!(
        run.step(&mut partial, &mut fetch),
        Err(DownloadError::Store("a read failed"))
    );
    assert_eq!(run.progress(), 1);
    assert_eq!((partial.inner.done, partial.inner.records.len()), (3, 0));
    let mut steps = Vec::new();
    loop {
        let step = run.step(&mut partial, &mut fetch).unwrap();
        steps.push(step);
        if matches!(step, Step::Done(_)) {
            break;
        }
    }
    let report = Report {
        kept: 3,
        fetched: 1,
    };
    assert_eq!(
        steps,
        [
            Step::Checked(1),
            Step::Checked(2),
            Step::Fetched(3),
            Step::Done(report)
        ]
    );
    assert_eq!(partial.inner.records, [4]);
    assert_eq!(partial.inner.bytes, target);
}

/// The same through `download`: the failed read is its error, and the next
/// call keeps every recorded chunk.
#[test]
fn a_failed_read_on_resume_costs_download_no_chunks() {
    let target = blob(3 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        fail_read: Some((0, "a read failed")),
        alone: true,
        ..Watched::default()
    };
    partial.inner.bytes = target[..2 * MIN_CHUNK as usize].to_vec();
    partial.inner.done = 2;
    let mut server = Server::new(target.clone());
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Err(DownloadError::Store("a read failed"))
    );
    assert_eq!(partial.inner.done, 2);
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 2,
            fetched: 1
        })
    );
    assert_eq!(fetches_of(&server, 0), 0);
}

/// A partial with no bytes where its mark names two chunks, as one another
/// holder removed leaves it: the read is short, so the chunks are fetched
/// again rather than the resume failing at each try.
#[test]
fn a_partial_shorter_than_its_mark_is_fetched_again() {
    let target = blob(2 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Memory {
        done: 2,
        ..Memory::default()
    };
    assert_eq!(
        partial.read(0, &mut [0u8; 4]),
        Err(DownloadError::Store(SHORT_READ))
    );
    let mut server = Server::new(target.clone());
    assert_eq!(
        run(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 0,
            fetched: 2
        })
    );
    assert_eq!(partial.records, [0, 1, 2]);
    assert_eq!(partial.bytes, target);
}

/// A shared partial another writer changes after every check: a stepped run
/// starts again from the mark `MAX_RESTARTS` times, then stops for good with
/// `KEEPS_CHANGING`, so a caller that steps until done, trying again after
/// each error, ends. Every later step answers the same and reads, writes and
/// fetches nothing.
#[test]
fn a_partial_that_keeps_changing_stops_a_stepped_run_for_good() {
    let target = blob(2 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        flip_every: Some((2, 5)),
        ..Watched::default()
    };
    let mut server = Server::new(target);
    let mut fetch =
        |range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| server.fetch(range, sink);
    let mut run = Download::start(list, &mut partial).unwrap();
    let mut errors = Vec::new();
    for steps in 0.. {
        assert!(steps < 100, "the run never stopped");
        match run.step(&mut partial, &mut fetch) {
            Ok(Step::Done(_)) => panic!("a partial that changed moved in"),
            Ok(_) => {}
            Err(DownloadError::Store(KEEPS_CHANGING)) => break,
            Err(e) => errors.push(e),
        }
    }
    let restarts = MAX_RESTARTS as usize;
    assert_eq!(
        errors,
        vec![DownloadError::Store(CHANGED_ON_DISK); restarts]
    );
    // Each restart fetched both chunks again, the first run too.
    assert_eq!(partial.writes, 2 * (restarts + 1));
    assert!(!partial.inner.finished);
    let (read, writes) = (partial.read, partial.writes);
    assert_eq!(
        run.step(&mut partial, &mut fetch),
        Err(DownloadError::Store(KEEPS_CHANGING))
    );
    assert_eq!((partial.read, partial.writes), (read, writes));
}

/// A read during the check before the move-in that fails other than short,
/// as a store that fails for a moment does, is passed on: nothing moves in,
/// the mark stays, and the next call reads every chunk back, keeps them all,
/// and moves the target in without fetching any again.
#[test]
fn a_failed_read_during_the_check_is_passed_on_and_the_mark_kept() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        fail_read: Some((2 * MIN_CHUNK, "a read failed")),
        ..Watched::default()
    };
    let mut server = Server::new(target.clone());
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Err(DownloadError::Store("a read failed"))
    );
    assert!(!partial.inner.finished);
    assert_eq!(partial.inner.records, [1, 2, 3, 4]);
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 4,
            fetched: 0
        })
    );
    assert!(partial.inner.finished);
    let fetches: Vec<usize> = (0..4).map(|c| fetches_of(&server, c)).collect();
    assert_eq!(fetches, [1, 1, 1, 1]);
}

/// `updated`'s case when the store refuses a model's sealed copy for its
/// hash: the partial, held alone, is whole and its mark names every chunk,
/// but a byte of chunk 2 changed on disk. The mark is kept, and the next run
/// reads back and keeps chunks 0 and 1, and fetches again only from chunk 2,
/// the first bad one.
#[test]
fn a_whole_partial_with_a_bad_chunk_is_fetched_again_only_from_it() {
    let target = blob(5 * MIN_CHUNK as usize + 9);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        alone: true,
        ..Watched::default()
    };
    partial.inner.bytes = target.clone();
    partial.inner.done = list.count();
    partial.inner.bytes[2 * MIN_CHUNK as usize + 17] ^= 0x08;
    let mut server = Server::new(target.clone());
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 2,
            fetched: 4
        })
    );
    assert_eq!(partial.inner.records, [2, 3, 4, 5, 6]);
    let fetches: Vec<usize> = (0..6).map(|c| fetches_of(&server, c)).collect();
    assert_eq!(fetches, [0, 0, 1, 1, 1, 1]);
    assert_eq!(partial.inner.bytes, target);
    assert!(partial.inner.finished);
}
