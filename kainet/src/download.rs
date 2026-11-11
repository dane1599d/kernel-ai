//! Verified downloads with resume (plan p5-networking, task 6; ADR 0008 §9):
//! a target fetched a chunk at a time by `Range`, each chunk checked against
//! its signed `kai-chunks 1` hash before it is written, a checkpoint kept
//! after each, and the whole target's SHA-256 checked at the end before it is
//! moved into place.
//!
//! A resume after a reboot reads back the chunks already written, checks each
//! again and rebuilds the whole-target hash from them, then fetches from the
//! first unchecked chunk: at most the chunk that was in flight is fetched
//! again. A chunk on disk that no longer matches is fetched again too. A
//! resume after a cut in the same process carries the hash over instead
//! ([`Progress`]). Where the bytes live is the caller's ([`Partial`]):
//! kaifs's `.partial/` in the OS (ADR 0011 §5), memory in tests.

use crate::chunks::Chunks;
use crate::http::{HttpError, Range, Url};
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

/// Fetches a URL's range, passing its bytes on: how the server answered, and
/// the URL that answered, after any redirects ([`crate::http::Fetched`]'s
/// `url`). [`Direct::range`] asks it.
pub type Get<'a> = dyn FnMut(
        &Url,
        Range,
        &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<(Answer, Url), HttpError>
    + 'a;

/// A target's ranges, asked straight of the URL that answered the last one.
///
/// A release asset is reached through a redirect (GitHub's releases host to
/// its assets host), so asking the target's own URL for each range costs the
/// redirect's connection every chunk. Once a range is answered with a 206,
/// [`Direct`] asks the URL that answered it for the next. That URL may stop
/// answering, as GitHub signs its asset URLs for minutes:
/// - a range it answers with anything but a 206, before a byte is passed on,
///   is asked of the target's own URL again, once;
/// - an error forgets it, so the next range, a resume's too, goes by the
///   target's URL.
///
/// Nothing is trusted more for it: each hop is still checked against the
/// caller's hosts by the fetch, and each chunk against the signed list.
#[derive(Debug, Clone)]
pub struct Direct {
    target: Url,
    answered: Option<Url>,
}

impl Direct {
    pub fn new(target: Url) -> Self {
        Self {
            target,
            answered: None,
        }
    }

    /// Where the next range is asked.
    pub fn url(&self) -> &Url {
        self.answered.as_ref().unwrap_or(&self.target)
    }

    /// Fetches `range` with `get`, as [`download`]'s fetcher does.
    pub fn range(
        &mut self,
        range: Range,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
        get: &mut Get,
    ) -> Result<Answer, HttpError> {
        if let Some(url) = self.answered.take() {
            let mut passed = false;
            let (answer, at) = get(&url, range, &mut |bytes| {
                passed = true;
                sink(bytes)
            })?;
            if answer.status == 206 || passed {
                self.answered = (answer.status == 206).then_some(at);
                return Ok(answer);
            }
        }
        let (answer, at) = get(&self.target, range, sink)?;
        self.answered = (answer.status == 206).then_some(at);
        Ok(answer)
    }
}

/// What a download did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Chunks already on disk: read back and checked again, or carried in
    /// a [`Progress`] from this process's run before.
    pub kept: usize,
    /// Chunks fetched this run.
    pub fetched: usize,
}

/// What a run of [`download_on`] keeps for the next run in the same process:
/// how many chunks it checked and wrote, and the whole target's hash over
/// them. A resume given it reads back only the chunks recorded past it, so a
/// cut does not cost re-reading everything written before it.
///
/// It trusts what this process wrote no more than a run with no cut does,
/// which hashes each chunk as fetched and never reads it back. A reboot has
/// no `Progress`, so its resume reads back and checks every chunk recorded,
/// and a mark behind the `Progress`, or another target's, starts it over.
#[derive(Clone, Default)]
pub struct Progress {
    /// The target's signed SHA-256, which this progress is for.
    blob: [u8; 32],
    done: usize,
    whole: Sha256,
}

impl Progress {
    /// Chunks checked and written, as this process knows.
    pub fn done(&self) -> usize {
        self.done
    }
}

/// Downloads the target `list` describes into `partial`, fetching with
/// `fetch`. Call it again after an error to resume; [`download_on`] resumes
/// without reading back what this process wrote.
pub fn download(
    list: &Chunks,
    partial: &mut impl Partial,
    fetch: &mut Fetcher,
) -> Result<Report, DownloadError> {
    download_on(list, partial, fetch, &mut Progress::default())
}

/// [`download`], carrying `progress` from this process's run before.
pub fn download_on(
    list: &Chunks,
    partial: &mut impl Partial,
    fetch: &mut Fetcher,
    progress: &mut Progress,
) -> Result<Report, DownloadError> {
    let recorded = partial.checkpoint()?.min(list.count());
    if progress.blob != list.blob || progress.done > recorded {
        *progress = Progress {
            blob: list.blob,
            ..Progress::default()
        };
    }
    if progress.done < recorded {
        let mut buf = vec![0u8; usize::try_from(list.chunk).unwrap_or(usize::MAX)];
        while progress.done < recorded {
            let (start, end) = list.range(progress.done);
            let bytes = &mut buf[..(end - start) as usize];
            partial.read(start, bytes)?;
            let hash: [u8; 32] = Sha256::digest(&*bytes).into();
            if hash != list.hashes[progress.done] {
                break;
            }
            progress.whole.update(&*bytes);
            progress.done += 1;
        }
        if progress.done < recorded {
            partial.record(progress.done)?;
        }
    }
    let kept = progress.done;
    for i in kept..list.count() {
        let (start, end) = list.range(i);
        let want = (end - start) as usize;
        let mut got: Vec<u8> = Vec::with_capacity(want);
        let range = Range {
            first: start,
            last: Some(end - 1),
        };
        let answer = fetch(range, &mut |bytes| {
            if got.len() + bytes.len() > want {
                return Err(HttpError::TooLarge);
            }
            got.extend_from_slice(bytes);
            Ok(())
        })
        .map_err(|e| match e {
            HttpError::TooLarge => DownloadError::Length(i),
            e => DownloadError::Fetch(e),
        })?;
        if answer.status != 206 {
            return Err(DownloadError::Status(answer.status, answer.retry_after));
        }
        if got.len() != want {
            return Err(DownloadError::Length(i));
        }
        let hash: [u8; 32] = Sha256::digest(&got).into();
        if hash != list.hashes[i] {
            return Err(DownloadError::Chunk(i));
        }
        partial.write(start, &got)?;
        partial.record(i + 1)?;
        progress.whole.update(&got);
        progress.done = i + 1;
    }
    let blob: [u8; 32] = progress.whole.clone().finalize().into();
    if blob != list.blob {
        return Err(DownloadError::Whole);
    }
    partial.finish()?;
    Ok(Report {
        kept,
        fetched: list.count() - kept,
    })
}
