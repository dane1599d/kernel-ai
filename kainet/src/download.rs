//! Verified downloads with resume (plan p5-networking, task 6; ADR 0008 §9):
//! a target fetched a chunk at a time by `Range`, each chunk checked against
//! its signed `kai-chunks 1` hash before it is written, a checkpoint kept
//! after each, and the whole target's SHA-256 checked at the end before it is
//! moved into place.
//!
//! A resume, after a cut or a reboot, reads back the chunks already written,
//! checks each again and rebuilds the whole-target hash from them, then
//! fetches from the first unchecked chunk: at most the chunk that was in
//! flight is fetched again. A chunk on disk that no longer matches is fetched
//! again too. Where the bytes live is the caller's ([`Partial`]): kaifs's
//! `.partial/` in the OS (ADR 0011 §5), memory in tests.

use crate::chunks::Chunks;
use crate::http::{HttpError, Range};
use alloc::vec;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

/// Why a download stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum DownloadError {
    /// Chunk `i` came, but its hash is not the signed one: nothing of it was
    /// written.
    Chunk(usize),
    /// Every chunk matched, but the whole target's SHA-256 is not the signed
    /// one: the list is wrong, and the target is not moved in.
    Whole,
    /// Chunk `i` came back shorter or longer than its range.
    Length(usize),
    /// The server answered a chunk's range with this status, not 206, and
    /// with this `Retry-After` in seconds if it gave one (a 429 or a 503
    /// says when to come back).
    Status(u16, Option<u32>),
    /// The fetch failed: a cut, a refused certificate, a timeout.
    Fetch(HttpError),
    /// The store failed.
    Store(&'static str),
}

/// Where a download's bytes go: a partial target and its checkpoint.
pub trait Partial {
    /// How many chunks the last run checked and wrote, as recorded.
    fn checkpoint(&mut self) -> Result<usize, DownloadError>;
    /// Records that the first `done` chunks are checked and written. It must
    /// be durable before it returns.
    fn record(&mut self, done: usize) -> Result<(), DownloadError>;
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError>;
    /// Fills `buf` from `offset`, which was written before.
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError>;
    /// The whole target is checked: move it into place.
    fn finish(&mut self) -> Result<(), DownloadError>;
}

/// How the server answered a range: [`crate::http::Fetched`] gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub retry_after: Option<u32>,
}

impl Answer {
    /// What a 206 says.
    pub const PARTIAL: Answer = Answer {
        status: 206,
        retry_after: None,
    };
}

impl From<&crate::http::Fetched> for Answer {
    fn from(got: &crate::http::Fetched) -> Self {
        Answer {
            status: got.head.status,
            retry_after: got.head.retry_after,
        }
    }
}

/// Fetches `range` of the target, passing its bytes on: how the server
/// answered. Only a 206's bytes are taken.
pub type Fetcher<'a> = dyn FnMut(Range, &mut dyn FnMut(&[u8]) -> Result<(), HttpError>) -> Result<Answer, HttpError>
    + 'a;

/// What a download did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Chunks already on disk and checked again.
    pub kept: usize,
    /// Chunks fetched this run.
    pub fetched: usize,
}

/// Downloads the target `list` describes into `partial`, fetching with
/// `fetch`: a [`Download`]'s steps in a loop. Call it again after an error to
/// resume.
pub fn download(
    list: &Chunks,
    partial: &mut impl Partial,
    fetch: &mut Fetcher,
) -> Result<Report, DownloadError> {
    let mut run = Download::start(list.clone(), partial)?;
    loop {
        if let Step::Done(report) = run.step(partial, fetch)? {
            return Ok(report);
        }
    }
}

/// What one [`Download::step`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Chunk `i`, on disk from an earlier run, was read back and checked again.
    Checked(usize),
    /// Chunk `i` was fetched, checked, written and recorded.
    Fetched(usize),
    /// The whole target checked out and was moved into place.
    Done(Report),
}

/// A download run a chunk at a time, so that its caller can do other work
/// between chunks, such as answering requests: `updated` steps one in its
/// serve loop. The steps are [`download`]'s, in its order. A step that fails
/// leaves the run where it was, so stepping again tries the same chunk, or
/// the same `finish`; only [`DownloadError::Whole`] is final, as the list is
/// wrong. A new run, from the checkpoint, does as well after a restart. It
/// holds one chunk's buffer for the run, for the read-backs and the fetches
/// alike.
pub struct Download {
    list: Chunks,
    whole: Sha256,
    buf: Vec<u8>,
    /// Chunks the checkpoint names that are not yet read back; cut to where
    /// the first one that no longer matches is.
    recorded: usize,
    /// Chunks read back and checked again.
    kept: usize,
    /// The next chunk to read back or fetch.
    next: usize,
    done: Option<Report>,
}

impl Download {
    /// A run of the target `list` describes, from `partial`'s checkpoint.
    pub fn start(list: Chunks, partial: &mut impl Partial) -> Result<Self, DownloadError> {
        let recorded = partial.checkpoint()?.min(list.count());
        let buf = vec![0u8; usize::try_from(list.chunk).unwrap_or(usize::MAX)];
        Ok(Self {
            list,
            whole: Sha256::new(),
            buf,
            recorded,
            kept: 0,
            next: 0,
            done: None,
        })
    }

    pub fn list(&self) -> &Chunks {
        &self.list
    }

    /// Chunks read back or fetched so far, of `list().count()`.
    pub fn progress(&self) -> usize {
        self.next
    }

    /// The next chunk: one read back and checked again while the checkpoint
    /// names any, then one fetched, then the whole target's hash and
    /// `partial.finish()`. Once done, it answers [`Step::Done`] again.
    pub fn step(
        &mut self,
        partial: &mut impl Partial,
        fetch: &mut Fetcher,
    ) -> Result<Step, DownloadError> {
        if let Some(report) = self.done {
            return Ok(Step::Done(report));
        }
        let i = self.next;
        if i < self.recorded {
            let (start, end) = self.list.range(i);
            let bytes = &mut self.buf[..(end - start) as usize];
            // A chunk that cannot be read back, as one cut short by a crash,
            // counts as one that no longer matches.
            let read = partial.read(start, bytes).is_ok();
            let hash: [u8; 32] = Sha256::digest(&*bytes).into();
            if read && hash == self.list.hashes[i] {
                self.whole.update(&*bytes);
                self.kept += 1;
                self.next += 1;
                return Ok(Step::Checked(i));
            }
            // It no longer matches: it and every chunk after it are fetched
            // again.
            self.recorded = i;
            partial.record(i)?;
        }
        if i < self.list.count() {
            self.fetch_chunk(i, partial, fetch)?;
            self.next += 1;
            return Ok(Step::Fetched(i));
        }
        // A clone, so a `finish` that fails can be stepped again.
        let blob: [u8; 32] = self.whole.clone().finalize().into();
        if blob != self.list.blob {
            return Err(DownloadError::Whole);
        }
        partial.finish()?;
        let report = Report {
            kept: self.kept,
            fetched: self.list.count() - self.kept,
        };
        self.done = Some(report);
        Ok(Step::Done(report))
    }

    /// Chunk `i` fetched, checked against its signed hash, then written and
    /// recorded; nothing of it is written if any check fails.
    fn fetch_chunk(
        &mut self,
        i: usize,
        partial: &mut impl Partial,
        fetch: &mut Fetcher,
    ) -> Result<(), DownloadError> {
        let (start, end) = self.list.range(i);
        let want = (end - start) as usize;
        let buf = &mut self.buf[..want];
        let mut got = 0;
        let range = Range {
            first: start,
            last: Some(end - 1),
        };
        let answer = fetch(range, &mut |bytes| {
            let to = got + bytes.len();
            if to > want {
                return Err(HttpError::TooLarge);
            }
            buf[got..to].copy_from_slice(bytes);
            got = to;
            Ok(())
        })
        .map_err(|e| match e {
            HttpError::TooLarge => DownloadError::Length(i),
            e => DownloadError::Fetch(e),
        })?;
        if answer.status != 206 {
            return Err(DownloadError::Status(answer.status, answer.retry_after));
        }
        if got != want {
            return Err(DownloadError::Length(i));
        }
        let hash: [u8; 32] = Sha256::digest(&*buf).into();
        if hash != self.list.hashes[i] {
            return Err(DownloadError::Chunk(i));
        }
        partial.write(start, buf)?;
        partial.record(i + 1)?;
        self.whole.update(&*buf);
        Ok(())
    }
}
