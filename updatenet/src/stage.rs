//! Streaming the release into the idle slot (ADR 0008 sections 4 and 12): the
//! *stage* event around `kainet`'s verified download.
//!
//! The slot is marked Updating, and that is made durable, before the first byte is
//! written: a half-written slot never boots. The download then writes each chunk
//! only after it matches its signed hash and records its progress, so a stop of any
//! kind can be resumed from the first unchecked chunk. Only when every chunk, the
//! whole image's hash and the slot's read-back all match does the slot become a
//! trial ([`bootslot::stage_end`]), written to the boot variable last.
//!
//! A stop is one of two things. [`Stop::Retry`] is the network's: a cut, a
//! timeout, a connection or TLS transport that failed, a 429 or a 5xx. Nothing is
//! wrong with what was written, the slot stays Updating, and calling [`stage`]
//! again resumes. [`Stop::Bad`] is a refusal of what the server sent (or of the
//! server): a chunk that is not the signed one, a chunk of the wrong length, a
//! whole image that is not the signed one, an answer that is not a 206, a refused
//! certificate or redirect. The slot stays unbootable, nothing after it is
//! written, and a bad whole forgets the checkpoint (the list was wrong, so nothing
//! written under it is trusted).
//!
//! A download can span a long time and many retries, so once every chunk and the
//! read-back have checked out, and before the slot becomes a trial, the signed
//! metadata is asked again ([`stage`]'s `recheck`): the release must still be the
//! one the repository signs. If a newer release is signed, or this one was pulled,
//! that is [`Bad::Superseded`]; if the metadata cannot be confirmed (the repository
//! cannot be reached, or its metadata has expired), a [`Stop::Retry`], and the next
//! call finds every chunk written and goes straight to the check.

use crate::plan::{Job, Refused};
use crate::slot::{READ_BACK, SlotPartial};
use alloc::format;
use alloc::string::String;
use bootslot::{Nv, Slot};
use kainet::chunks::Chunks;
use kainet::download::{DownloadError, Fetcher, Report, download};
use kainet::http::HttpError;
use kainet::tls::TlsError;
use kaituf::meta::TargetFile;

/// The boot variable, which `stage` changes through the stage events and writes
/// back. A `set` is durable when it returns.
pub trait Boot {
    fn set(&mut self, nv: &Nv) -> Result<(), &'static str>;
}

/// What a staged update is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Staged {
    pub slot: Slot,
    pub release_seq: u32,
    pub report: Report,
}

/// Why the server's answer is refused: the slot stays unbootable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bad {
    /// Chunk `i` is not the signed one: none of it was written.
    Chunk(usize),
    /// Chunk `i` came shorter or longer than its range.
    Length(usize),
    /// Every chunk matched and the whole image did not: the list is wrong.
    Whole,
    /// The slot read back as other than it was written.
    ReadBack,
    /// The server answered a range with this status (not 206, and not one that
    /// asks to come back later).
    Status(u16),
    /// The fetch was refused: a certificate, a host off the repository's, a
    /// malformed answer.
    Fetch(String),
    /// The image checked out, and the signed metadata no longer names this release
    /// (a newer one is signed, or this one was pulled): it is not made a trial.
    Superseded,
}

/// Why `stage` stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Come back later and resume, after `after_s` seconds if the server said.
    Retry {
        after_s: Option<u32>,
        why: String,
    },
    Bad(Bad),
    /// The slot, the checkpoint or the boot variable failed.
    Store(&'static str),
}

impl Stop {
    /// In words, for the shell's answer.
    pub fn why(&self) -> String {
        match self {
            Self::Retry { after_s, why } => match after_s {
                Some(s) => format!("stopped ({why}); it resumes where it was, after {s} s"),
                None => format!("stopped ({why}); it resumes where it was"),
            },
            Self::Bad(Bad::Chunk(i)) => {
                format!("part {i} is not the signed one; the slot stays unbootable")
            }
            Self::Bad(Bad::Length(i)) => {
                format!("part {i} came the wrong length; the slot stays unbootable")
            }
            Self::Bad(Bad::Whole) => String::from(
                "every part matched but the image is not the signed one; the slot stays unbootable",
            ),
            Self::Bad(Bad::ReadBack) => {
                String::from("the slot reads back other than the signed image; it stays unbootable")
            }
            Self::Bad(Bad::Status(status)) => {
                format!("the server answered {status}; the slot stays unbootable")
            }
            Self::Bad(Bad::Fetch(why)) => {
                format!("the download was refused ({why}); the slot stays unbootable")
            }
            Self::Bad(Bad::Superseded) => String::from(
                "the repository no longer signs this release; the slot stays unbootable",
            ),
            Self::Store(why) => String::from(*why),
        }
    }
}

/// A network stop that a later call resumes, or a refusal of the server.
fn stop_of(error: DownloadError) -> Stop {
    match error {
        DownloadError::Chunk(i) => Stop::Bad(Bad::Chunk(i)),
        DownloadError::Length(i) => Stop::Bad(Bad::Length(i)),
        DownloadError::Whole => Stop::Bad(Bad::Whole),
        DownloadError::Status(status, after_s)
            if status == 429 || (500..=599).contains(&status) =>
        {
            Stop::Retry {
                after_s,
                why: format!("the server answered {status}"),
            }
        }
        DownloadError::Status(status, _) => Stop::Bad(Bad::Status(status)),
        DownloadError::Store(READ_BACK) => Stop::Bad(Bad::ReadBack),
        DownloadError::Store(why) => Stop::Store(why),
        DownloadError::Fetch(error) => match error {
            // A transport that failed under TLS (in the handshake or a record) is
            // the network's, like a cut; what rustls itself refuses (a certificate,
            // a record that does not decrypt, a downgrade) is not.
            HttpError::Truncated
            | HttpError::Timeout
            | HttpError::Connect(_)
            | HttpError::Tls(TlsError::Transport(_) | TlsError::Truncated) => Stop::Retry {
                after_s: None,
                why: format!("{error:?}"),
            },
            error => Stop::Bad(Bad::Fetch(format!("{error:?}"))),
        },
    }
}

/// The metadata asked again, now the image has checked out: whether the repository
/// still signs this release.
fn confirm(
    job: &Job,
    recheck: &mut dyn FnMut() -> Result<(TargetFile, TargetFile), Refused>,
) -> Result<(), Stop> {
    match recheck() {
        Ok((image, list)) if job.still_signed(&image, &list) => Ok(()),
        Err(Refused::Repository(why)) => Err(Stop::Retry {
            after_s: None,
            why: format!("the signed metadata could not be confirmed: {why}"),
        }),
        Ok(_) | Err(_) => Err(Stop::Bad(Bad::Superseded)),
    }
}

/// Streams `job`'s image into its slot. `list` is the chunk list [`Job::chunks`]
/// accepted; `fetch` fetches a range of the image ([`crate::net::Over::range`]).
///
/// The slot is marked Updating and the boot variable written before anything is
/// written to the slot. Calling it again after a [`Stop::Retry`], a reboot or a
/// crash resumes at the first unchecked chunk; after a [`Stop::Bad`] it starts the
/// image's download again from the checkpoint, which a bad chunk leaves where it
/// was. On success the slot is a trial of the release, at the boot variable.
///
/// `recheck` refreshes the signed metadata again ([`crate::plan::refresh`]) and
/// gives the pair it names. It is called once, after every chunk and the read-back
/// have checked out and before the slot becomes a trial; the pair must be the
/// job's (see [`Job::still_signed`]).
pub fn stage(
    job: &Job,
    list: &Chunks,
    nv: &mut Nv,
    boot: &mut dyn Boot,
    partial: &mut SlotPartial<'_>,
    recheck: &mut dyn FnMut() -> Result<(TargetFile, TargetFile), Refused>,
    fetch: &mut Fetcher<'_>,
) -> Result<Staged, Stop> {
    bootslot::stage_begin(nv, job.slot);
    boot.set(nv).map_err(Stop::Store)?;
    let report = match download(list, partial, fetch) {
        Ok(report) => report,
        Err(error) => {
            if error == DownloadError::Whole {
                partial.forget().map_err(Stop::Store)?;
            }
            return Err(stop_of(error));
        }
    };
    confirm(job, recheck)?;
    bootslot::stage_end(nv, job.slot, job.release.seq);
    boot.set(nv).map_err(Stop::Store)?;
    Ok(Staged {
        slot: job.slot,
        release_seq: job.release.seq,
        report,
    })
}
