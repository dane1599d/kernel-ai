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
//!
//! A partial not held by this downloader alone ([`Partial::held_alone`]) is
//! checked when it is moved in: just before [`Partial::finish`], every chunk
//! is read back and checked, and the whole target and its length. That is a
//! check at that moment, not a snapshot: another holder can still change the
//! bytes after it, so such a target is only as trustworthy as its folder's
//! other writers. `fs` cannot fence a folder from the other holders of its
//! `Dir`, nor move a file between two folders' handles, so a `.partial`
//! beside its target in a shared folder is checked this way.

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
    /// Whether only this downloader can change what it wrote: the partial
    /// lives where no other grant reaches (`updated`'s idle slot; the model
    /// store's `.partial/`, which only `dir=models` opens). When it is not,
    /// as for a file in a folder other holders of its `Dir` can write,
    /// [`download`] checks it when it is moved in ([`read_back`]): a check at
    /// that moment, not a snapshot, so the target is only as trustworthy as
    /// the folder's other writers.
    fn held_alone(&self) -> bool {
        false
    }
    /// The partial's length in bytes, now. A partial not
    /// [`Partial::held_alone`] must give it, as [`read_back`] holds it to the
    /// target's: the default refuses ([`NO_SIZE`]), so a partial that cannot
    /// say is never moved in.
    fn size(&mut self) -> Result<u64, DownloadError> {
        Err(DownloadError::Store(NO_SIZE))
    }
}

/// Why [`download`] refused to move a target in: read back before
/// [`Partial::finish`], a chunk no longer matched or could not be read, or
/// the partial was shorter than the target ([`DownloadError::Store`]). The
/// mark was set back to that chunk, so a resume fetches it and every chunk
/// after it again.
pub const CHANGED_ON_DISK: &str = "a chunk changed on disk after it was checked";
/// Why [`download`] refused to move a target in: read back before
/// [`Partial::finish`], every chunk matched but the partial is longer than
/// the target, as when another writer appended to it. `download` cannot cut
/// a partial, so the mark is left as it is: the caller removes the partial
/// and starts over.
pub const LONGER_ON_DISK: &str = "the partial is longer than its target";
/// Why [`download`] refused to move a target in: the partial is not held
/// alone and cannot give its length ([`Partial::size`]).
pub const NO_SIZE: &str = "the partial cannot give its length";

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
/// `fetch`. Call it again after an error to resume.
pub fn download(
    list: &Chunks,
    partial: &mut impl Partial,
    fetch: &mut Fetcher,
) -> Result<Report, DownloadError> {
    let mut whole = Sha256::new();
    let recorded = partial.checkpoint()?.min(list.count());
    let mut buf = vec![0u8; usize::try_from(list.chunk).unwrap_or(usize::MAX)];
    let mut kept = 0;
    while kept < recorded {
        let (start, end) = list.range(kept);
        let bytes = &mut buf[..(end - start) as usize];
        partial.read(start, bytes)?;
        let hash: [u8; 32] = Sha256::digest(&*bytes).into();
        if hash != list.hashes[kept] {
            break;
        }
        whole.update(&*bytes);
        kept += 1;
    }
    if kept < recorded {
        partial.record(kept)?;
    }
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
        whole.update(&got);
    }
    let blob: [u8; 32] = whole.finalize().into();
    if blob != list.blob {
        return Err(DownloadError::Whole);
    }
    if !partial.held_alone() {
        read_back(list, partial, &mut buf)?;
    }
    partial.finish()?;
    Ok(Report {
        kept,
        fetched: list.count() - kept,
    })
}

/// The check [`download`] makes when it moves in a partial not
/// [`Partial::held_alone`], just before [`Partial::finish`], and what a
/// caller that steps a download itself does at the same point: every chunk
/// read back and checked against the list, the whole target against its
/// signed hash, then the partial's length against the target's.
/// - A chunk that no longer matches, or cannot be read, sets the mark back to
///   it ([`CHANGED_ON_DISK`]), as does a partial found shorter.
/// - A partial longer than the target is refused ([`LONGER_ON_DISK`]), its
///   mark left as it is.
///
/// `buf` holds one chunk; a shorter one is replaced with one that does.
pub fn read_back(
    list: &Chunks,
    partial: &mut impl Partial,
    buf: &mut [u8],
) -> Result<(), DownloadError> {
    let need = usize::try_from(list.chunk).unwrap_or(usize::MAX);
    let mut own = Vec::new();
    let buf = match buf.len() >= need {
        true => buf,
        false => {
            own.resize(need, 0);
            own.as_mut_slice()
        }
    };
    let mut whole = Sha256::new();
    for i in 0..list.count() {
        let (start, end) = list.range(i);
        let bytes = &mut buf[..(end - start) as usize];
        let read = partial.read(start, bytes).is_ok();
        let hash: [u8; 32] = Sha256::digest(&*bytes).into();
        if !read || hash != list.hashes[i] {
            partial.record(i)?;
            return Err(DownloadError::Store(CHANGED_ON_DISK));
        }
        whole.update(&*bytes);
    }
    let blob: [u8; 32] = whole.finalize().into();
    if blob != list.blob {
        return Err(DownloadError::Whole);
    }
    let size = partial.size()?;
    if size > list.size {
        return Err(DownloadError::Store(LONGER_ON_DISK));
    }
    if size < list.size {
        let short = usize::try_from(size / list.chunk.max(1)).unwrap_or(0);
        partial.record(short.min(list.count()))?;
        return Err(DownloadError::Store(CHANGED_ON_DISK));
    }
    Ok(())
}
