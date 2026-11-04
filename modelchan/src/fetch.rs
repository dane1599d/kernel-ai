//! The model downloaded in the OS (P5 task 9, roadmap L7; ADR 0008 §9), apart
//! from the network: which of the signed `models` role's targets this machine
//! can take ([`offers`]), the one ask a download needs ([`ask`], P5 question
//! 6: it names the size and the host), and the job `updated` hands to
//! `kainet`'s verified download ([`plan`], [`Job`]): the pinned source, where
//! the bytes land while they come and where they are moved once whole.
//!
//! Before any byte is fetched, the job's chunk list (`kai-chunks 1`), which the
//! role carries as a signed sibling target `<sha256>.chunks` ([`Job::chunks`]),
//! must be the role's own target: its whole-blob SHA-256 and size are held to the
//! target's ([`Job::check_list`]). `kainet` then checks each 2 MiB chunk
//! against the list before it is written and the whole SHA-256 at the end, so
//! nothing reaches `/system/models` that the role did not sign.
//!
//! What fits is the same arithmetic xtask sizes its VMs by
//! ([`kabi::model::guest_mib`]): Qwen3-4B needs a guest of 4,160 MiB, so a
//! 4 GiB guest is offered Qwen3-1.7B and a 6 GiB one Qwen3-4B. The disk must
//! hold the target and [`DISK_MARGIN`] besides.

use crate::Sha256;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_models::{Signed, Store, hex, size};

/// Where the store keeps its models, each as `<sha256>.gguf`.
pub const MODELS_DIR: &str = "/system/models";
/// Where a download's bytes wait until they are whole (ADR 0011 §5).
pub const PARTIAL_DIR: &str = "/system/models/.partial";
/// Disk left free after a download, so the volume never fills to its last
/// byte: 256 MiB.
pub const DISK_MARGIN: u64 = 256 << 20;
/// The one port a source is reached on.
pub const HTTPS_PORT: u16 = 443;
/// The ending of the sibling target that holds a model's chunk list.
pub const CHUNKS_SUFFIX: &str = ".chunks";

/// What this machine has for a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Machine {
    /// Guest memory, in MiB.
    pub memory_mib: u64,
    /// Free bytes on the volume that holds `/system/models`.
    pub free_bytes: u64,
}

/// Whether a target can be downloaded here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The store holds it already.
    Stored,
    Fits,
    /// It needs a guest of `need_mib` and this one has `have_mib`.
    Memory {
        need_mib: u64,
        have_mib: u64,
    },
    /// It needs `need` bytes of disk, margin included, and `free` are free.
    Disk {
        need: u64,
        free: u64,
    },
}

/// One target the role signs, and whether it fits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer<'a> {
    pub target: &'a Signed,
    pub fit: Fit,
    /// The one to offer first: the largest that fits.
    pub recommended: bool,
}

/// Why a download does not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The role signs no target of that id.
    NotSigned(String),
    /// Its licence is not one the project takes.
    Licence(String),
    Stored,
    Memory {
        need_mib: u64,
        have_mib: u64,
    },
    Disk {
        need: u64,
        free: u64,
    },
    /// Its source is not a pinned `https` URL of a `.gguf` file.
    Source(&'static str),
    /// The chunk list is not the role's target.
    List(&'static str),
}

impl Refused {
    /// The refusal in words, for the shell's answer.
    pub fn why(&self) -> String {
        match self {
            Self::NotSigned(id) => format!("the signed models role names no {id}"),
            Self::Licence(l) => format!("its licence, {l}, is not one Kernel AI takes"),
            Self::Stored => String::from("the model store holds it already"),
            // In MiB: rounded to tenths of a GiB, 4,160 and 4,096 MiB read
            // alike.
            Self::Memory { need_mib, have_mib } => {
                format!("it needs {need_mib} MiB of memory and this machine has {have_mib} MiB")
            }
            Self::Disk { need, free } => format!(
                "it needs {} of disk and {} is free",
                size(*need),
                size(*free)
            ),
            Self::Source(why) | Self::List(why) => String::from(*why),
        }
    }
}

/// A target's source: an `https` URL at a pinned revision, Hugging Face's
/// `/<owner>/<repo>/resolve/<40 hex digits>/<file>.gguf`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub host: String,
    pub path: String,
}

impl Source {
    pub fn parse(url: &str) -> Result<Self, Refused> {
        let rest = url
            .strip_prefix("https://")
            .ok_or(Refused::Source("the source is not an https URL"))?;
        let (host, path) = rest
            .split_once('/')
            .ok_or(Refused::Source("the source has no path"))?;
        let host_ok = !host.is_empty()
            && host.len() <= 253
            && host
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
        if !host_ok {
            return Err(Refused::Source("the source's host is not a plain name"));
        }
        let parts: Vec<&str> = path.split('/').collect();
        let clean = parts.iter().all(|p| {
            !p.is_empty()
                && *p != "."
                && *p != ".."
                && p.bytes()
                    .all(|b| b.is_ascii_graphic() && !b"?#%\\".contains(&b))
        });
        let [_, _, "resolve", revision, file] = parts.as_slice() else {
            return Err(Refused::Source("the source is not a resolve URL"));
        };
        let pinned = revision.len() == 40
            && revision
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !clean || !pinned || !file.ends_with(".gguf") {
            return Err(Refused::Source("the source is not pinned to a revision"));
        }
        Ok(Self {
            host: String::from(host),
            path: format!("/{path}"),
        })
    }

    pub fn url(&self) -> String {
        format!("https://{}{}", self.host, self.path)
    }
}

/// How `target` fits `machine`, given what `store` holds.
pub fn fit(target: &Signed, store: &Store, machine: &Machine) -> Fit {
    if store.blobs.iter().any(|b| b.sha256 == target.sha256) {
        return Fit::Stored;
    }
    let need_mib = kabi::model::guest_mib(target.length);
    if need_mib > machine.memory_mib {
        return Fit::Memory {
            need_mib,
            have_mib: machine.memory_mib,
        };
    }
    let need = target.length.saturating_add(DISK_MARGIN);
    if need > machine.free_bytes {
        return Fit::Disk {
            need,
            free: machine.free_bytes,
        };
    }
    Fit::Fits
}

/// The targets of the signed role this OS may download, largest first, each
/// with whether it fits; the largest that fits is the one recommended. A target
/// whose licence the project does not take, or whose source is not pinned, is
/// left out.
pub fn offers<'a>(signed: &'a [Signed], store: &Store, machine: &Machine) -> Vec<Offer<'a>> {
    let mut offers: Vec<Offer<'a>> = signed
        .iter()
        .filter(|t| t.licence_ok() && Source::parse(&t.source).is_ok())
        .map(|target| Offer {
            target,
            fit: fit(target, store, machine),
            recommended: false,
        })
        .collect();
    offers.sort_by(|a, b| {
        b.target
            .length
            .cmp(&a.target.length)
            .then_with(|| a.target.id.cmp(&b.target.id))
    });
    if let Some(best) = offers.iter_mut().find(|o| o.fit == Fit::Fits) {
        best.recommended = true;
    }
    offers
}

/// The one ask a download needs (P5 question 6): what, how large, from where,
/// and that it is checked.
pub fn ask(target: &Signed, source: &Source) -> String {
    format!(
        "Download {} ({}) from {}? Each part is checked against the signed models role before it is kept.",
        target.name,
        size(target.length),
        source.host
    )
}

/// A download to run: what it fetches, from where, and where it lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub sha256: Sha256,
    pub length: u64,
    pub source: Source,
    /// Where the bytes wait while they come.
    pub partial: String,
    /// Where they are moved once whole.
    pub path: String,
    /// The role's sibling target that holds the model's `kai-chunks 1` list,
    /// named by the model's SHA-256 so it cannot be paired with another model:
    /// fetched from the repository host, then held to the model by
    /// [`Job::check_list`] before any range is fetched.
    pub chunks: String,
}

/// The download of target `id`, or why it does not start: it must be a target
/// the role signs, under a licence the project takes, with a pinned source,
/// not stored yet, and it must fit `machine`.
pub fn plan(signed: &[Signed], store: &Store, machine: &Machine, id: &str) -> Result<Job, Refused> {
    let target = signed
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| Refused::NotSigned(String::from(id)))?;
    if !target.licence_ok() {
        return Err(Refused::Licence(target.licence.clone()));
    }
    let source = Source::parse(&target.source)?;
    match fit(target, store, machine) {
        Fit::Fits => {}
        Fit::Stored => return Err(Refused::Stored),
        Fit::Memory { need_mib, have_mib } => return Err(Refused::Memory { need_mib, have_mib }),
        Fit::Disk { need, free } => return Err(Refused::Disk { need, free }),
    }
    let name = hex(&target.sha256);
    Ok(Job {
        id: target.id.clone(),
        sha256: target.sha256,
        length: target.length,
        source,
        partial: format!("{PARTIAL_DIR}/{name}.gguf"),
        path: format!("{MODELS_DIR}/{name}.gguf"),
        chunks: format!("{name}{CHUNKS_SUFFIX}"),
    })
}

impl Job {
    /// Holds a chunk list, as `kai-chunks 1` names its blob, to the role's
    /// target: the same SHA-256 and size, or no byte is fetched.
    pub fn check_list(&self, blob: &Sha256, size: u64) -> Result<(), Refused> {
        if *blob != self.sha256 {
            return Err(Refused::List("the chunk list is for another blob"));
        }
        if size != self.length {
            return Err(Refused::List("the chunk list's size is not the target's"));
        }
        Ok(())
    }

    /// Whether the download may reach `host` on `port`: the source's host on
    /// 443 only, since a pinned source is plain `https` with no port. A
    /// redirect elsewhere (Hugging Face's CDN) needs the caller's `NetDest` to
    /// name it, and is the caller's to allow.
    pub fn reaches(&self, host: &str, port: u16) -> bool {
        port == HTTPS_PORT && host == self.source.host
    }
}
