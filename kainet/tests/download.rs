//! `kai-chunks 1` against hostile lists, and the downloader through cuts,
//! reboots, flipped bytes, a wrong list and a corrupted partial, with a store
//! and a server in memory.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kainet::chunks::{CHUNK, Chunks, MIN_CHUNK};
use kainet::download::{
    Answer, CHANGED_ON_DISK, Direct, Download, DownloadError, Get, KEEPS_CHANGING, LONGER_ON_DISK,
    MAX_RESTARTS, NO_SIZE, Partial, Report, SHORT_READ, Step, download, read_back,
};
use kainet::http::{HttpError, Range, Url};
use std::cell::{Cell, RefCell};

fn blob(n: usize) -> Vec<u8> {
    let mut rng = hostile::XorShift::new(n as u64);
    (0..n).map(|_| rng.next() as u8).collect()
}

#[test]
fn a_list_renders_and_parses_back() {
    for size in [
        0,
        1,
        MIN_CHUNK as usize,
        MIN_CHUNK as usize + 1,
        5 * MIN_CHUNK as usize - 3,
    ] {
        let list = Chunks::of(&blob(size), MIN_CHUNK);
        assert_eq!(list.count() as u64, (size as u64).div_ceil(MIN_CHUNK));
        assert_eq!(
            Chunks::parse(list.render().as_bytes()),
            Ok(list.clone()),
            "size {size}"
        );
    }
    let list = Chunks::of(&blob(3 * CHUNK as usize), CHUNK);
    assert!(list.render().starts_with("kai-chunks 1\nblob "));
    assert!(list.render().contains("\nchunk 2097152\n"));
}

#[test]
fn hostile_lists_are_refused_by_name() {
    let good = Chunks::of(&blob(2 * MIN_CHUNK as usize + 5), MIN_CHUNK).render();
    let hash = "a".repeat(64);
    for (text, why) in [
        (
            good.replace("kai-chunks 1", "kai-chunks 2"),
            "not a kai-chunks 1 list",
        ),
        (good.trim_end().to_string(), "ends with a newline"),
        (good.replace('\n', "\r\n"), "not a kai-chunks 1 list"),
        (
            good.to_uppercase()
                .replace("KAI-CHUNKS", "kai-chunks")
                .replace("BLOB", "blob")
                .replace("CHUNK ", "chunk "),
            "hash",
        ),
        (format!("{good}{hash}\n"), "more hashes"),
        (
            good.rsplitn(3, '\n').nth(2).unwrap().to_string() + "\n",
            "fewer hashes",
        ),
        (good.replace("chunk 65536", "chunk 65535"), "power of two"),
        (good.replace("chunk 65536", "chunk 32768"), "power of two"),
        (
            good.replace("chunk 65536", "chunk 33554432"),
            "power of two",
        ),
        (good.replace("chunk 65536", "chunk 065536"), "decimal"),
        (good.replace(" 131077\n", " +131077\n"), "decimal"),
        (good.replace(" 131077\n", " 131077 x\n"), "blob line"),
        (
            format!("kai-chunks 1\nblob {hash} 99999999999999\nchunk 65536\n"),
            "16,384",
        ),
        (
            format!("kai-chunks 1\nblob {hash} 99999999999999999999\nchunk 65536\n"),
            "decimal",
        ),
        ("kai-chunks 1\n".to_string(), "no blob line"),
        (format!("kai-chunks 1\nblob {hash} 0\n"), "no chunk line"),
    ] {
        let got = Chunks::parse(text.as_bytes());
        assert!(
            matches!(got, Err(message) if message.contains(why)),
            "{why}: {got:?}"
        );
    }
    assert!(Chunks::parse(&vec![b'a'; (1 << 20) + 1]).is_err());
    assert!(Chunks::parse(b"kai-chunks 1\n\xff\n").is_err());
}

#[test]
fn mutated_lists_never_panic() {
    let good = Chunks::of(&blob(3 * MIN_CHUNK as usize + 9), MIN_CHUNK).render();
    hostile::fuzz(good.as_bytes(), 0x6b61_6963, 20_000, |bytes| {
        let _ = Chunks::parse(bytes);
    });
    hostile::truncations(good.as_bytes(), |cut, bytes| {
        let parsed = Chunks::parse(bytes);
        assert_eq!(parsed.is_ok(), cut == good.len(), "cut at {cut}");
    });
}

/// A partial target in memory, with its checkpoint; finished when moved in.
#[derive(Default)]
struct Memory {
    bytes: Vec<u8>,
    done: usize,
    finished: bool,
    records: Vec<usize>,
    /// How many `finish` calls fail before one moves the target in.
    failed_finishes: u32,
}

impl Partial for Memory {
    fn checkpoint(&mut self) -> Result<usize, DownloadError> {
        Ok(self.done)
    }

    fn record(&mut self, done: usize) -> Result<(), DownloadError> {
        self.done = done;
        self.records.push(done);
        Ok(())
    }

    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        let end = offset as usize + bytes.len();
        if self.bytes.len() < end {
            self.bytes.resize(end, 0);
        }
        self.bytes[offset as usize..end].copy_from_slice(bytes);
        Ok(())
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
        let src = self
            .bytes
            .get(offset as usize..offset as usize + buf.len())
            .ok_or(DownloadError::Store(SHORT_READ))?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        if self.failed_finishes > 0 {
            self.failed_finishes -= 1;
            return Err(DownloadError::Store("the move failed"));
        }
        self.finished = true;
        Ok(())
    }

    fn size(&mut self) -> Result<u64, DownloadError> {
        Ok(self.bytes.len() as u64)
    }
}

/// A server of `target` that cuts once at each offset in `cuts`, and flips
/// the byte at `flip` every time; the ranges it served.
struct Server {
    target: Vec<u8>,
    cuts: Vec<u64>,
    flip: Option<u64>,
    served: Vec<(u64, u64)>,
}

impl Server {
    fn new(target: Vec<u8>) -> Self {
        Self {
            target,
            cuts: Vec::new(),
            flip: None,
            served: Vec::new(),
        }
    }

    fn fetch(
        &mut self,
        range: Range,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<Answer, HttpError> {
        let first = range.first;
        let last = range.last.unwrap_or(self.target.len() as u64 - 1);
        self.served.push((first, last + 1));
        let mut bytes = self.target[first as usize..=last as usize].to_vec();
        if let Some(at) = self.flip.filter(|at| (first..=last).contains(at)) {
            bytes[(at - first) as usize] ^= 1;
        }
        if let Some(i) = self.cuts.iter().position(|c| (first..=last).contains(c)) {
            let cut = self.cuts.remove(i);
            sink(&bytes[..(cut - first) as usize])?;
            return Err(HttpError::Truncated);
        }
        sink(&bytes)?;
        Ok(Answer::PARTIAL)
    }
}

fn run(list: &Chunks, partial: &mut Memory, server: &mut Server) -> Result<Report, DownloadError> {
    download(list, partial, &mut |range, sink| server.fetch(range, sink))
}

#[test]
fn a_target_comes_chunk_by_chunk_and_is_moved_in_whole() {
    let target = blob(5 * MIN_CHUNK as usize + 77);
    let list = Chunks::of(&target, MIN_CHUNK);
    let (mut partial, mut server) = (Memory::default(), Server::new(target.clone()));
    assert_eq!(
        run(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 0,
            fetched: 6
        })
    );
    assert!(partial.finished);
    assert_eq!(partial.bytes, target);
    assert_eq!(partial.records, [1, 2, 3, 4, 5, 6]);
    assert_eq!(server.served[5], (5 * MIN_CHUNK, 5 * MIN_CHUNK + 77));
}

#[test]
fn cuts_at_forty_and_eighty_percent_fetch_only_the_chunk_in_flight_again() {
    let target = blob(10 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let len = target.len() as u64;
    let (mut partial, mut server) = (Memory::default(), Server::new(target.clone()));
    server.cuts = vec![len * 4 / 10 + 100, len * 8 / 10 + 100];
    let mut errors = 0;
    let report = loop {
        match run(&list, &mut partial, &mut server) {
            Ok(report) => break report,
            Err(DownloadError::Fetch(HttpError::Truncated)) => errors += 1,
            Err(other) => panic!("{other:?}"),
        }
    };
    assert_eq!(
        (errors, report),
        (
            2,
            Report {
                kept: 8,
                fetched: 2
            }
        )
    );
    assert_eq!(partial.bytes, target);
    // Eleven chunk fetches for ten chunks plus one per cut, never more.
    assert_eq!(server.served.len(), 12);
    let fetched_twice = server
        .served
        .iter()
        .filter(|r| r.0 == 4 * MIN_CHUNK)
        .count();
    assert_eq!(fetched_twice, 2);
}

#[test]
fn a_reboot_resumes_from_the_checkpoint_rechecking_what_is_on_disk() {
    let target = blob(6 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let (mut partial, mut server) = (Memory::default(), Server::new(target.clone()));
    server.cuts = vec![3 * MIN_CHUNK + 5];
    assert!(run(&list, &mut partial, &mut server).is_err());
    // The machine restarts: the partial target and its checkpoint survive,
    // the downloader's state does not.
    let mut after = Memory {
        bytes: partial.bytes.clone(),
        done: partial.done,
        ..Memory::default()
    };
    let mut server = Server::new(target.clone());
    assert_eq!(
        run(&list, &mut after, &mut server),
        Ok(Report {
            kept: 3,
            fetched: 3
        })
    );
    assert_eq!(server.served.first(), Some(&(3 * MIN_CHUNK, 4 * MIN_CHUNK)));
    assert_eq!(after.bytes, target);
}

#[test]
fn a_flipped_byte_is_refused_by_its_chunk_and_never_written() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let (mut partial, mut server) = (Memory::default(), Server::new(target.clone()));
    server.flip = Some(2 * MIN_CHUNK + 9);
    assert_eq!(
        run(&list, &mut partial, &mut server),
        Err(DownloadError::Chunk(2))
    );
    assert_eq!(
        (partial.done, partial.bytes.len(), partial.finished),
        (2, 2 * MIN_CHUNK as usize, false)
    );
}

#[test]
fn a_list_whose_whole_hash_is_wrong_is_never_moved_in() {
    let target = blob(3 * MIN_CHUNK as usize);
    let mut list = Chunks::of(&target, MIN_CHUNK);
    list.blob[0] ^= 1;
    let (mut partial, mut server) = (Memory::default(), Server::new(target));
    assert_eq!(
        run(&list, &mut partial, &mut server),
        Err(DownloadError::Whole)
    );
    assert!(!partial.finished);
}

#[test]
fn a_chunk_that_went_bad_on_disk_is_fetched_again() {
    let target = blob(5 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Memory {
        bytes: target[..4 * MIN_CHUNK as usize].to_vec(),
        done: 4,
        ..Memory::default()
    };
    partial.bytes[MIN_CHUNK as usize + 1] ^= 0x80;
    let mut server = Server::new(target.clone());
    assert_eq!(
        run(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 1,
            fetched: 4
        })
    );
    assert_eq!(partial.bytes, target);
}

#[test]
fn a_server_that_sends_too_much_or_too_little_is_refused() {
    let target = blob(2 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let calls = Cell::new(0);
    let mut long = |_: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        calls.set(calls.get() + 1);
        sink(&vec![0; MIN_CHUNK as usize + 1])?;
        Ok(Answer::PARTIAL)
    };
    assert_eq!(
        download(&list, &mut Memory::default(), &mut long),
        Err(DownloadError::Length(0))
    );
    let mut short = |_: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        sink(&[1, 2, 3])?;
        Ok(Answer::PARTIAL)
    };
    assert_eq!(
        download(&list, &mut Memory::default(), &mut short),
        Err(DownloadError::Length(0))
    );
    assert_eq!(calls.get(), 1);
}

/// A status other than 206 comes back as itself, with when to come back,
/// and nothing is written (C's review of #662).
#[test]
fn a_status_other_than_206_is_passed_through_with_its_retry_after() {
    let target = blob(2 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut busy = |_: Range, _: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        Ok(Answer {
            status: 429,
            retry_after: Some(30),
        })
    };
    let mut partial = Memory::default();
    assert_eq!(
        download(&list, &mut partial, &mut busy),
        Err(DownloadError::Status(429, Some(30)))
    );
    assert_eq!((partial.done, partial.bytes.len()), (0, 0));
}

/// [`Memory`] as a store with other writers, or one held alone: it counts
/// the bytes read back. After the `after`th write another writer can flip a
/// byte at `flip`, or `append` a byte; `flip_every` flips one after every
/// `n`th write; a read at `fail_read` fails once; and `says_size` can make it
/// give a length other than its own.
#[derive(Default)]
struct Watched {
    inner: Memory,
    alone: bool,
    read: usize,
    writes: usize,
    flip: Option<(usize, usize)>,
    flip_every: Option<(usize, usize)>,
    append: Option<usize>,
    fail_read: Option<u64>,
    says_size: Option<u64>,
}

impl Partial for Watched {
    fn checkpoint(&mut self) -> Result<usize, DownloadError> {
        self.inner.checkpoint()
    }
    fn record(&mut self, done: usize) -> Result<(), DownloadError> {
        self.inner.record(done)
    }
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        self.inner.write(offset, bytes)?;
        self.writes += 1;
        if let Some((after, at)) = self.flip
            && self.writes == after
        {
            self.inner.bytes[at] ^= 1;
            self.flip = None;
        }
        if let Some((every, at)) = self.flip_every
            && self.writes.is_multiple_of(every)
        {
            self.inner.bytes[at] ^= 1;
        }
        if self.append == Some(self.writes) {
            self.inner.bytes.push(7);
            self.append = None;
        }
        Ok(())
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
        self.read += buf.len();
        if self.fail_read == Some(offset) {
            self.fail_read = None;
            return Err(DownloadError::Store("a read failed"));
        }
        self.inner.read(offset, buf)
    }
    fn finish(&mut self) -> Result<(), DownloadError> {
        self.inner.finish()
    }
    fn held_alone(&self) -> bool {
        self.alone
    }
    fn size(&mut self) -> Result<u64, DownloadError> {
        Ok(self.says_size.unwrap_or(self.inner.bytes.len() as u64))
    }
}

fn run_watched(
    list: &Chunks,
    partial: &mut Watched,
    server: &mut Server,
) -> Result<Report, DownloadError> {
    download(list, partial, &mut |range, sink| server.fetch(range, sink))
}

/// A partial others could write is read back once, whole, before it moves
/// in, even when nothing was cut.
#[test]
fn a_shared_partial_is_read_back_once_before_it_moves_in() {
    let target = blob(5 * MIN_CHUNK as usize + 77);
    let list = Chunks::of(&target, MIN_CHUNK);
    let (mut partial, mut server) = (Watched::default(), Server::new(target.clone()));
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 0,
            fetched: 6
        })
    );
    assert!(partial.inner.finished);
    assert_eq!(partial.read, target.len());
}

/// A partial held alone, as `updated`'s slot is, is not read back by
/// `download`.
#[test]
fn a_partial_held_alone_is_not_read_back() {
    let target = blob(5 * MIN_CHUNK as usize + 77);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        alone: true,
        ..Watched::default()
    };
    let mut server = Server::new(target.clone());
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 0,
            fetched: 6
        })
    );
    assert!(partial.inner.finished);
    assert_eq!(partial.read, 0);
}

/// Another writer flips a byte of chunk 1 after the last chunk is written:
/// every chunk checked out as fetched, yet the target is not moved in. The
/// mark goes back to chunk 1, and a resume fetches it again and finishes.
#[test]
fn a_chunk_changed_on_disk_after_its_check_is_never_moved_in() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        flip: Some((4, MIN_CHUNK as usize + 9)),
        ..Watched::default()
    };
    let mut server = Server::new(target.clone());
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Err(DownloadError::Store(CHANGED_ON_DISK))
    );
    assert!(!partial.inner.finished);
    assert_eq!(partial.inner.records.last(), Some(&1));
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 1,
            fetched: 3
        })
    );
    assert!(partial.inner.finished);
    assert_eq!(partial.inner.bytes, target);
    let chunk_one = server.served.iter().filter(|r| r.0 == MIN_CHUNK).count();
    assert_eq!(chunk_one, 2);
}

/// The same flip on a partial held alone is not looked for: the promise is
/// the holder's. (`updated`'s slot hashes itself in `finish`.)
#[test]
fn a_partial_held_alone_is_trusted_to_be_its_holders() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        alone: true,
        flip: Some((4, MIN_CHUNK as usize + 9)),
        ..Watched::default()
    };
    let mut server = Server::new(target);
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 0,
            fetched: 4
        })
    );
    assert_eq!(partial.read, 0);
}

/// How many times chunk `chunk` was fetched, by its first byte.
fn fetches_of(server: &Server, chunk: u64) -> usize {
    server
        .served
        .iter()
        .filter(|r| r.0 == chunk * MIN_CHUNK)
        .count()
}

/// A byte appended after the last write: every chunk still matches, but the
/// partial is longer than the target, so it is not moved in, and its mark is
/// left for the caller, who removes it.
#[test]
fn a_tail_appended_after_the_last_write_is_never_moved_in() {
    let target = blob(3 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        append: Some(3),
        ..Watched::default()
    };
    let mut server = Server::new(target);
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Err(DownloadError::Store(LONGER_ON_DISK))
    );
    assert!(!partial.inner.finished);
    assert_eq!(partial.inner.done, 3);
}

/// A zero-length target with a partial another holder planted: nothing is
/// fetched or read, and the planted bytes are still not moved in. An empty
/// partial for it is.
#[test]
fn a_planted_partial_for_an_empty_target_is_never_moved_in() {
    let list = Chunks::of(&[], MIN_CHUNK);
    assert_eq!(list.count(), 0);
    let mut planted = Watched::default();
    planted.inner.bytes = b"planted".to_vec();
    let mut server = Server::new(Vec::new());
    assert_eq!(
        run_watched(&list, &mut planted, &mut server),
        Err(DownloadError::Store(LONGER_ON_DISK))
    );
    assert!(!planted.inner.finished);
    let mut empty = Watched::default();
    assert_eq!(
        run_watched(&list, &mut empty, &mut server),
        Ok(Report {
            kept: 0,
            fetched: 0
        })
    );
    assert!(empty.inner.finished);
}

/// A flip in the short last chunk is found too: the mark goes back to it,
/// and a resume fetches only it again.
#[test]
fn a_flip_in_the_short_last_chunk_is_never_moved_in() {
    let target = blob(4 * MIN_CHUNK as usize + 77);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        flip: Some((5, 4 * MIN_CHUNK as usize + 70)),
        ..Watched::default()
    };
    let mut server = Server::new(target.clone());
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Err(DownloadError::Store(CHANGED_ON_DISK))
    );
    assert_eq!(partial.inner.done, 4);
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 4,
            fetched: 1
        })
    );
    assert_eq!(partial.inner.bytes, target);
    assert_eq!(fetches_of(&server, 4), 2);
}

/// A read that fails during the check counts as a chunk that changed: the
/// mark goes back to it, and a resume fetches it and every chunk after it
/// again, so a resume never fails the same way.
#[test]
fn a_read_error_during_the_check_fetches_from_that_chunk_again() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        fail_read: Some(2 * MIN_CHUNK),
        ..Watched::default()
    };
    let mut server = Server::new(target.clone());
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Err(DownloadError::Store(CHANGED_ON_DISK))
    );
    assert_eq!(partial.inner.done, 2);
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Ok(Report {
            kept: 2,
            fetched: 2
        })
    );
    assert!(partial.inner.finished);
    let fetches: Vec<usize> = (0..4).map(|c| fetches_of(&server, c)).collect();
    assert_eq!(fetches, [1, 1, 2, 2]);
}

/// A partial found shorter than the target after its chunks were read sets
/// the mark back to the first chunk it no longer holds whole.
#[test]
fn a_partial_found_short_sets_the_mark_back() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Watched {
        says_size: Some(2 * MIN_CHUNK + 5),
        ..Watched::default()
    };
    let mut server = Server::new(target);
    assert_eq!(
        run_watched(&list, &mut partial, &mut server),
        Err(DownloadError::Store(CHANGED_ON_DISK))
    );
    assert_eq!(partial.inner.done, 2);
    assert!(!partial.inner.finished);
}

/// A partial that is not held alone and cannot give its length is never
/// moved in: the default `size` refuses.
#[test]
fn a_shared_partial_that_cannot_give_its_length_is_never_moved_in() {
    struct NoSize(Memory);
    impl Partial for NoSize {
        fn checkpoint(&mut self) -> Result<usize, DownloadError> {
            self.0.checkpoint()
        }
        fn record(&mut self, done: usize) -> Result<(), DownloadError> {
            self.0.record(done)
        }
        fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
            self.0.write(offset, bytes)
        }
        fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
            self.0.read(offset, buf)
        }
        fn finish(&mut self) -> Result<(), DownloadError> {
            self.0.finish()
        }
    }
    let target = blob(2 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = NoSize(Memory::default());
    let mut server = Server::new(target);
    assert_eq!(
        download(&list, &mut partial, &mut |r, sink| server.fetch(r, sink)),
        Err(DownloadError::Store(NO_SIZE))
    );
    assert!(!partial.0.finished);
}

/// `read_back` given a buffer shorter than a chunk does not panic: it takes
/// one that fits.
#[test]
fn read_back_takes_a_buffer_that_fits_when_given_a_short_one() {
    let target = blob(3 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let mut partial = Memory {
        bytes: target,
        done: 3,
        ..Memory::default()
    };
    assert_eq!(read_back(&list, &mut partial, &mut [0u8; 4]), Ok(()));
}

/// A release asset's URL and where its redirect leads, as GitHub's go.
const RELEASE: &str = "https://releases.kai.test/o/r/releases/download/t/blob";
const ASSET: &str = "https://assets.kai.test/assets/t/blob?sig=1";

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

/// A server for [`Direct`]: the release URL redirects to the asset, which
/// answers each range in turn as `asset` says, a status or an error. It
/// records each URL asked.
fn asked_of<'a>(
    asked: &'a RefCell<Vec<String>>,
    asset: &'a RefCell<Vec<Result<u16, HttpError>>>,
) -> Box<Get<'a>> {
    Box::new(
        move |at: &Url, _: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
            let text = format!("https://{}{}", at.host, at.path);
            asked.borrow_mut().push(text.clone());
            let status = if text == RELEASE {
                206
            } else {
                asset.borrow_mut().remove(0)?
            };
            if status == 206 {
                sink(&[7; 4])?;
            }
            let answer = Answer {
                status,
                retry_after: None,
            };
            Ok((answer, url(ASSET)))
        },
    )
}

const RANGE: Range = Range {
    first: 0,
    last: Some(3),
};

fn take(direct: &mut Direct, get: &mut Get) -> Result<(u16, usize), HttpError> {
    let mut got = 0;
    let answer = direct.range(
        RANGE,
        &mut |b| {
            got += b.len();
            Ok(())
        },
        get,
    )?;
    Ok((answer.status, got))
}

/// After the first range, every range skips the redirect and its connection.
#[test]
fn direct_asks_the_url_that_answered_after_the_first_range() {
    let asked = RefCell::new(Vec::new());
    let asset = RefCell::new(vec![Ok(206), Ok(206)]);
    let mut get = asked_of(&asked, &asset);
    let mut direct = Direct::new(url(RELEASE));
    for _ in 0..3 {
        assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    }
    let asset_path = "https://assets.kai.test/assets/t/blob?sig=1";
    assert_eq!(*asked.borrow(), [RELEASE, asset_path, asset_path]);
    assert_eq!(direct.url().host, "assets.kai.test");
}

/// A signed asset URL that has expired answers 403: that range is asked of
/// the release URL again, once, and its new asset URL is kept.
#[test]
fn an_expired_asset_url_is_asked_of_the_target_again_once() {
    let asked = RefCell::new(Vec::new());
    let asset = RefCell::new(vec![Ok(403), Ok(206)]);
    let mut get = asked_of(&asked, &asset);
    let mut direct = Direct::new(url(RELEASE));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    let hosts: Vec<bool> = asked.borrow().iter().map(|a| a == RELEASE).collect();
    assert_eq!(hosts, [true, false, true, false]);
}

/// An error, a cut say, forgets the asset URL: the next range, a resume's
/// too, goes by the release URL.
#[test]
fn an_error_sends_the_next_range_by_the_target() {
    let asked = RefCell::new(Vec::new());
    let asset = RefCell::new(vec![Err(HttpError::Truncated)]);
    let mut get = asked_of(&asked, &asset);
    let mut direct = Direct::new(url(RELEASE));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    assert_eq!(take(&mut direct, &mut get), Err(HttpError::Truncated));
    assert_eq!(direct.url().host, "releases.kai.test");
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    let hosts: Vec<bool> = asked.borrow().iter().map(|a| a == RELEASE).collect();
    assert_eq!(hosts, [true, false, true]);
}

/// A status that is not 206 from the release URL itself comes back as it
/// is, asked once, and nothing is kept.
#[test]
fn a_target_that_does_not_answer_206_is_asked_once() {
    let asked = RefCell::new(Vec::new());
    let mut get = |at: &Url, _: Range, _: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        asked.borrow_mut().push(at.host.clone());
        let answer = Answer {
            status: 404,
            retry_after: None,
        };
        Ok((answer, at.clone()))
    };
    let mut direct = Direct::new(url(RELEASE));
    assert_eq!(take(&mut direct, &mut get), Ok((404, 0)));
    assert_eq!(asked.borrow().len(), 1);
    assert_eq!(direct.url().host, "releases.kai.test");
}

/// A whole download through [`Direct`]: one request by the release URL, the
/// rest straight to the asset, and the target checks out.
#[test]
fn a_download_through_direct_redirects_once() {
    let target = blob(4 * MIN_CHUNK as usize);
    let list = Chunks::of(&target, MIN_CHUNK);
    let asked = RefCell::new(Vec::new());
    let mut get = |at: &Url, range: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        asked.borrow_mut().push(at.host.clone());
        let end = range.last.map_or(target.len(), |last| last as usize + 1);
        sink(&target[range.first as usize..end])?;
        Ok((Answer::PARTIAL, url(ASSET)))
    };
    let mut direct = Direct::new(url(RELEASE));
    let mut partial = Memory::default();
    let report = download(&list, &mut partial, &mut |range, sink| {
        direct.range(range, sink, &mut get)
    });
    assert_eq!(
        report,
        Ok(Report {
            kept: 0,
            fetched: 4
        })
    );
    assert_eq!(
        *asked.borrow(),
        [
            "releases.kai.test",
            "assets.kai.test",
            "assets.kai.test",
            "assets.kai.test"
        ]
    );
}

/// An asset URL that passes bytes on and then answers other than 206 is not
/// asked again: those bytes are already the caller's, and `download` refuses
/// the status.
#[test]
fn bytes_passed_on_before_a_status_are_not_asked_for_again() {
    let asked = RefCell::new(Vec::new());
    let mut get = |at: &Url, _: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        asked.borrow_mut().push(at.host.clone());
        sink(&[7; 4])?;
        let status = if asked.borrow().len() == 2 { 200 } else { 206 };
        let answer = Answer {
            status,
            retry_after: None,
        };
        Ok((answer, url(ASSET)))
    };
    let mut direct = Direct::new(url(RELEASE));
    assert_eq!(take(&mut direct, &mut get), Ok((206, 4)));
    assert_eq!(take(&mut direct, &mut get), Ok((200, 4)));
    assert_eq!(direct.url().host, "releases.kai.test");
    assert_eq!(*asked.borrow(), ["releases.kai.test", "assets.kai.test"]);
}

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
        fail_read: Some(MIN_CHUNK),
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
        fail_read: Some(0),
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
