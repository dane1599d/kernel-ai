//! `kai-chunks 1` against hostile lists, and the downloader through cuts,
//! reboots, flipped bytes, a wrong list and a corrupted partial, with a store
//! and a server in memory.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kainet::chunks::{CHUNK, Chunks, MIN_CHUNK};
use kainet::download::{DownloadError, Partial, Report, download};
use kainet::http::{HttpError, Range};
use std::cell::Cell;

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
            .ok_or(DownloadError::Store("read past the end"))?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        self.finished = true;
        Ok(())
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
    ) -> Result<u64, HttpError> {
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
        Ok(bytes.len() as u64)
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
        Ok(0)
    };
    assert_eq!(
        download(&list, &mut Memory::default(), &mut long),
        Err(DownloadError::Length(0))
    );
    let mut short = |_: Range, sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>| {
        sink(&[1, 2, 3])?;
        Ok(3)
    };
    assert_eq!(
        download(&list, &mut Memory::default(), &mut short),
        Err(DownloadError::Length(0))
    );
    assert_eq!(calls.get(), 1);
}
