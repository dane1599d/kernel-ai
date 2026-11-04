//! What an update is, and whether this machine may take it (ADR 0008 sections 2, 7
//! and 12): the release the signed `targets` role names, held to anti-rollback and
//! to the idle slot, and the job it becomes.
//!
//! A release is two signed targets, `slot.img` and its sibling `slot.img.chunks`.
//! `slot.img`'s `custom.kai` names its `release_seq` (the number updates are
//! ordered by, never a version) and the `tag` of the GitHub Release its file is an
//! asset of; with consistent snapshots the file is `<sha256>.slot.img` under that
//! tag, and the chunk list's `<sha256>.slot.img.chunks`.

use alloc::format;
use alloc::string::String;
use bootslot::{Reason, Record, Slot};
use kainet::chunks::{CHUNK, Chunks};
use kaipkg::online::Hosts;
use kaituf::meta::TargetFile;
use kaituf::updater::{Cache, Remote, Updater, check_target};

/// The release image, as `updated` looks it up (the update disk's name too).
pub const TARGET: &str = "slot.img";
/// Its chunk list, a sibling target.
pub const CHUNKS_TARGET: &str = "slot.img.chunks";
/// Longest release tag.
pub const MAX_TAG: usize = 64;

/// The release a `slot.img` target names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Only rises (ADR 0008 section 2).
    pub seq: u32,
    /// The GitHub Release its asset is under.
    pub tag: String,
    pub length: u64,
    pub sha256: [u8; 32],
}

/// Why an update does not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The signed metadata does not name this target.
    NotSigned(&'static str),
    /// The metadata did not verify, or could not be fetched: why, as `kaituf` says.
    Repository(String),
    /// A target lacks what it must carry, or carries a malformed value.
    Field(&'static str),
    /// Release `seq` is not newer than `newest`, which this machine has run or
    /// committed (anti-rollback).
    Rollback { seq: u32, newest: u32 },
    /// There is no slot to write: a boot running in R must name one.
    Slot(&'static str),
    /// The image is longer than the slot.
    Room { need: u64, room: u64 },
    /// The slot holds this release already, staged.
    Staged(u32),
    /// The chunk list is not the target's.
    List(&'static str),
}

impl Refused {
    /// In words, for the shell's answer.
    pub fn why(&self) -> String {
        match self {
            Self::NotSigned(name) => format!("the repository does not sign {name}"),
            Self::Repository(why) => format!("the repository was refused: {why}"),
            Self::Field(why) | Self::Slot(why) | Self::List(why) => String::from(*why),
            Self::Rollback { seq, newest } => {
                format!("release {seq} is not newer than {newest} (anti-rollback)")
            }
            Self::Room { need, room } => {
                format!("the image is {need} bytes and the slot holds {room}")
            }
            Self::Staged(seq) => format!("release {seq} is staged already"),
        }
    }
}

fn repository(error: kaituf::Error) -> Refused {
    Refused::Repository(format!("{error}"))
}

/// A plain tag: letters, digits, dots, hyphens and underscores, not starting with a
/// dot (so never `.` or `..`).
fn tag_ok(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= MAX_TAG
        && !tag.starts_with('.')
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn sha256_of(target: &TargetFile) -> Result<[u8; 32], Refused> {
    let (_, digest) = target
        .hashes
        .iter()
        .find(|(alg, _)| alg == "sha256")
        .ok_or(Refused::Field("a target has no sha256"))?;
    <[u8; 32]>::try_from(digest.as_slice()).map_err(|_| Refused::Field("a sha256 is not 32 bytes"))
}

/// The release a `slot.img` target names.
pub fn release_of(target: &TargetFile) -> Result<Release, Refused> {
    let kai = &target.custom["kai"];
    let seq = kai["release_seq"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or(Refused::Field("slot.img names no release_seq"))?;
    let tag = kai["tag"]
        .as_str()
        .filter(|tag| tag_ok(tag))
        .ok_or(Refused::Field("slot.img names no plain release tag"))?;
    Ok(Release {
        seq,
        tag: String::from(tag),
        length: target.length,
        sha256: sha256_of(target)?,
    })
}

/// The signed metadata, refreshed from `remote` (against the root the device
/// holds and the roles it cached, at `now` seconds since 1970), and the two
/// targets of the release it names. Metadata that does not verify, is expired,
/// rolls back or is missing is [`Refused::Repository`].
pub fn refresh(
    remote: &mut dyn Remote,
    cache: &mut dyn Cache,
    now: u64,
) -> Result<(TargetFile, TargetFile), Refused> {
    let mut updater = Updater::new(remote, cache, now).map_err(repository)?;
    updater.refresh().map_err(repository)?;
    let image = updater
        .target(TARGET)
        .map_err(repository)?
        .ok_or(Refused::NotSigned(TARGET))?;
    let list = updater
        .target(CHUNKS_TARGET)
        .map_err(repository)?
        .ok_or(Refused::NotSigned(CHUNKS_TARGET))?;
    Ok((image, list))
}

/// What this machine is: the boot record, which slot runs, and the room the idle
/// slot has.
#[derive(Clone, Copy, Debug)]
pub struct Machine<'a> {
    pub record: &'a Record,
    /// The slot this boot runs.
    pub running: Slot,
    /// In R, which system slot to write (`:reinstall`); ignored in A or B, which
    /// write the other one.
    pub into: Option<Slot>,
    /// Bytes the slot holds.
    pub room: u64,
}

impl Machine<'_> {
    /// The slot an update is written into: the other system slot, or in R the one
    /// the caller named.
    pub fn slot(&self) -> Result<Slot, Refused> {
        match (self.running, self.into) {
            (Slot::A, _) => Ok(Slot::B),
            (Slot::B, _) => Ok(Slot::A),
            (Slot::R, Some(slot @ (Slot::A | Slot::B))) => Ok(slot),
            (Slot::R, _) => Err(Refused::Slot("a boot in R must name slot A or B")),
        }
    }

    /// The newest release this machine has run or committed: the floor an update
    /// must be above. A running slot counts even on trial; in R the committed
    /// slots do.
    fn newest(&self) -> u32 {
        let floor = self.record.min_release_seq;
        let held = |slot: Slot| self.record.slot(slot).release_seq;
        match self.running {
            Slot::R => [Slot::A, Slot::B]
                .into_iter()
                .filter(|slot| self.record.slot(*slot).successful)
                .map(held)
                .fold(floor, u32::max),
            running => floor.max(held(running)),
        }
    }
}

/// An update to run: the release, the slot it goes into, where its bytes and its
/// chunk list are, and the hosts it may reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub release: Release,
    pub slot: Slot,
    pub image_url: String,
    pub list_url: String,
    /// The signed target the chunk list must be.
    pub list: TargetFile,
    pub hosts: Hosts,
}

/// The update of `image`, with its chunk list `list` (both from [`refresh`]), or why
/// it does not start: the release must be newer than every release this machine
/// has committed, fit the idle slot, and not be staged there already.
pub fn plan(
    image: &TargetFile,
    list: &TargetFile,
    machine: &Machine<'_>,
    hosts: &Hosts,
) -> Result<Job, Refused> {
    let release = release_of(image)?;
    let slot = machine.slot()?;
    let newest = machine.newest();
    if release.seq <= newest {
        return Err(Refused::Rollback {
            seq: release.seq,
            newest,
        });
    }
    if release.length == 0 || release.length > machine.room {
        return Err(Refused::Room {
            need: release.length,
            room: machine.room,
        });
    }
    let state = machine.record.slot(slot);
    if state.release_seq == release.seq && state.reason == Reason::None && state.priority > 0 {
        return Err(Refused::Staged(release.seq));
    }
    if list.length == 0 || list.length > kainet::chunks::MAX_LIST as u64 {
        return Err(Refused::List("the chunk list is empty or over 1 MiB"));
    }
    let list_sha = sha256_of(list)?;
    let url = |sha: &[u8; 32], name: &str| {
        format!(
            "{}{}/{}.{name}",
            hosts.releases,
            release.tag,
            kaituf::hex(sha)
        )
    };
    Ok(Job {
        image_url: url(&release.sha256, TARGET),
        list_url: url(&list_sha, CHUNKS_TARGET),
        release,
        slot,
        list: list.clone(),
        hosts: hosts.clone(),
    })
}

impl Job {
    /// The chunk list from the bytes fetched for it: they must be the signed
    /// target's (its length and hashes), and the list must be for this release's
    /// bytes, in 2 MiB chunks, or no byte of the image is fetched.
    pub fn chunks(&self, bytes: &[u8]) -> Result<Chunks, Refused> {
        check_target(&self.list, bytes)
            .map_err(|_| Refused::List("the chunk list is not the signed one"))?;
        let list = Chunks::parse(bytes).map_err(Refused::List)?;
        if list.blob != self.release.sha256 {
            return Err(Refused::List("the chunk list is for another image"));
        }
        if list.size != self.release.length {
            return Err(Refused::List("the chunk list's size is not the image's"));
        }
        if list.chunk != CHUNK {
            return Err(Refused::List("the chunk list's chunks are not 2 MiB"));
        }
        Ok(list)
    }

    /// Whether the metadata, refreshed again, still signs this job's release: the
    /// same `slot.img` (sequence, tag, length and hash) and the same chunk list.
    pub fn still_signed(&self, image: &TargetFile, list: &TargetFile) -> bool {
        release_of(image).is_ok_and(|release| release == self.release) && *list == self.list
    }

    /// Whether the update may reach `host` on `port`: the repository's hosts on 443.
    pub fn reaches(&self, host: &str, port: u16) -> bool {
        self.hosts.allows(host, port)
    }
}
