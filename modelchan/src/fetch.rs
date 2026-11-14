//! The model downloaded in the OS (P5 task 9, roadmap L7; ADR 0008 §9), apart
//! from the network: which of the signed `models` role's targets this machine
//! can take ([`offers`]), the one ask a download needs ([`ask`], P5 question
//! 6: it names the size and the host), and the job `updated` hands to
//! `kainet`'s verified download ([`plan`], [`Job`]): the pinned source, the
//! hosts it may reach, where the bytes land while they come and where they are
//! moved once whole.
//!
//! Before any byte is fetched, the job's chunk list (`kai-chunks 1`), which the
//! role carries as a signed sibling target `<sha256>.chunks` ([`Job::chunks`]),
//! must be the role's own target: its whole-blob SHA-256 and size are held to
//! the target's, in 2 MiB chunks ([`Job::check_list`]). `kainet` then checks
//! each chunk against the list before it is written and the whole SHA-256 at
//! the end, so nothing reaches `/system/models` that the role did not sign.
//!
//! A model host's resolve URL does not serve the bytes: it redirects to the
//! host's file storage. `kainet` checks every hop, so a job reaches its source's
//! host and that host's storage hosts ([`Hosts`]), on 443, and no other.
//!
//! What fits is the same arithmetic xtask sizes its VMs by
//! ([`kabi::model::guest_mib`]): Qwen3-4B needs a guest of 4,160 MiB, so a
//! 4 GiB guest is offered Qwen3-1.7B and a 6 GiB one Qwen3-4B. The disk must
//! hold what is still to come, then the store's sealed copy of the whole file,
//! and [`DISK_MARGIN`] besides ([`disk_need`]).

use crate::Sha256;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_models::{Signed, Store, hex, size};
use fsproto::{Error, Reply};

/// Where the store keeps its models, each as `<sha256>.gguf`.
pub const MODELS_DIR: &str = "/system/models";
/// Where a download's bytes wait until they are whole (ADR 0011 §5).
pub const PARTIAL_DIR: &str = "/system/models/.partial";
/// [`PARTIAL_DIR`] beneath the store's root, the `Dir` `updated` holds.
pub const PARTIAL_IN_STORE: &str = ".partial";

/// A target's partial file beneath the store's root:
/// `.partial/<sha256>.partial`, beside its mark `<sha256>.mark`, as
/// `user::kaifs` names a download it holds.
pub fn partial_name(sha256: &Sha256) -> String {
    format!("{PARTIAL_IN_STORE}/{}.partial", hex(sha256))
}
/// Disk left free after a download, so the volume never fills to its last
/// byte: 256 MiB.
pub const DISK_MARGIN: u64 = 256 << 20;
/// The one port a source is reached on.
pub const HTTPS_PORT: u16 = 443;
/// The ending of the sibling target that holds a model's chunk list.
pub const CHUNKS_SUFFIX: &str = ".chunks";
/// The chunk size a model's list must use (P5's 2 MiB, `kainet::chunks::CHUNK`):
/// `kainet` keeps two buffers of one chunk, so a larger chunk would cost
/// `updated` that much more memory.
pub const MODEL_CHUNK: u64 = 2 << 20;
/// The longest host name.
const MAX_HOST: usize = 253;

/// A host models come from, and the hosts its resolve URLs redirect to for
/// the bytes. A storage entry is a host name, or a suffix starting with `.`
/// that any name ending in it on a label boundary matches (`.cdn.hf.co`
/// matches `us.aws.cdn.hf.co`, not `evilcdn.hf.co` or `cdn.hf.co` itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hosts {
    pub source: &'static str,
    pub storage: &'static [&'static str],
}

/// Hugging Face: `huggingface.co`'s resolve URLs redirect to its file storage.
/// From a GitHub runner the pinned Qwen3-4B URL answered 302 to
/// `us.aws.cdn.hf.co` (#654's tls-hosts job, run 41878520332), a regional
/// name, so the storage is pinned as the `.cdn.hf.co` suffix rather than a name
/// another region would not get (the manager's call: this suffix, for the model
/// download's `NetDest` alone). The bytes are checked against the signed chunk
/// list whichever host serves them; the hosts limit where a download may go.
pub const HUGGING_FACE: Hosts = Hosts {
    source: "huggingface.co",
    storage: &[".cdn.hf.co"],
};

/// The HTTPS mock's model host and the storage host its resolve URLs redirect
/// to (`netcore::fixture`'s `models.kai.test` and `assets.kai.test`), which
/// `updated` takes in place of [`HUGGING_FACE`] only when the kernel's
/// `model-download` case plans it with `fetch=test`.
pub const FIXTURE: Hosts = Hosts {
    source: "models.kai.test",
    storage: &["assets.kai.test"],
};

/// The guest's memory in MiB, as [`kabi::model::guest_mib`] sizes it, from the
/// bytes of RAM the kernel's memory map lists (`kabi::Metrics::ram_frames`):
/// the firmware keeps a little of what the VM was given for itself, so it is
/// rounded up to [`kabi::model::GUEST_STEP_MIB`], the step VMs are sized in. A
/// VM of 4,160 MiB that lists 4,150 reads as 4,160 and is offered Qwen3-4B; a 4
/// GiB one reads as 4,096 and is not.
pub fn guest_mib_of(ram_bytes: u64) -> u64 {
    ram_bytes
        .div_ceil(1 << 20)
        .next_multiple_of(kabi::model::GUEST_STEP_MIB)
}

/// Whether `host` is `entry`: the same name, or, for an entry starting with
/// `.`, a DNS name under it.
fn matches(entry: &str, host: &str) -> bool {
    match entry.strip_prefix('.') {
        Some(_) => host.ends_with(entry) && host_ok(host),
        None => host == entry,
    }
}

/// What this machine has for a model.
#[derive(Clone, Copy, Debug)]
pub struct Machine<'a> {
    /// Guest memory, in MiB.
    pub memory_mib: u64,
    /// Free bytes on the volume that holds `/system/models`.
    pub free_bytes: u64,
    /// What the store holds.
    pub store: &'a Store,
    /// Bytes already in `.partial/` for a target, by its SHA-256: a resume
    /// needs disk for the rest and the copy, not for what is held.
    pub partials: &'a [(Sha256, u64)],
    /// The hosts models may come from.
    pub hosts: &'a [Hosts],
}

impl Machine<'_> {
    fn held(&self, sha256: &Sha256) -> u64 {
        self.partials
            .iter()
            .find(|(s, _)| s == sha256)
            .map_or(0, |(_, n)| *n)
    }

    fn hosts_of(&self, source: &Source) -> Option<&Hosts> {
        self.hosts.iter().find(|h| h.source == source.host)
    }
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
    /// It needs `need` more bytes of disk, margin included, and `free` are free.
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
    /// Its source is not a pinned `https` URL of a `.gguf` file on a host
    /// models come from.
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

/// Whether `host` is a DNS name as `kainet`'s URLs take one: lower-case labels
/// of letters, digits and hyphens, none empty or over 63 bytes, none starting
/// or ending with a hyphen, and a last label that is not all digits, so no IP
/// literal gets through.
fn host_ok(host: &str) -> bool {
    let labels_ok = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    });
    let named = host
        .rsplit('.')
        .next()
        .is_some_and(|tld| !tld.bytes().all(|b| b.is_ascii_digit()));
    !host.is_empty() && host.len() <= MAX_HOST && labels_ok && named
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
        if !host_ok(host) {
            return Err(Refused::Source("the source's host is not a DNS name"));
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

/// The free disk a download of `length` bytes needs with `held` of them in
/// `.partial/` already: the rest of the partial file, then a second copy of
/// the whole, since the store keeps only sealed blobs, which `fs` hashes as it
/// writes them, so the partial file is copied in, not renamed, and goes only
/// once the copy is kept; and [`DISK_MARGIN`]. Once the partial file is whole,
/// `disk_need(length, length)` is what the copy still needs.
pub fn disk_need(length: u64, held: u64) -> u64 {
    (length - held.min(length))
        .saturating_add(length)
        .saturating_add(DISK_MARGIN)
}

/// Why the store refused a downloaded model's sealed copy at `SealedFinish`,
/// after `updated` pushed the whole length into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyRefused {
    /// `fs` hashed the copy as it was written, and its SHA-256 is not the
    /// model's (`Invalid`): the partial it was read from no longer holds the
    /// bytes the download checked. Its mark is kept: the next
    /// `models download` reads back every chunk it names, checks each against
    /// the signed list, and fetches again from the first that changed, not
    /// the whole model.
    BadPartial,
    /// Any other refusal, as a full disk: the store's, not the partial's.
    Store(Reply),
}

impl CopyRefused {
    /// The refusal `SealedFinish` answered with `reply`, which is not `Done`.
    pub fn of(reply: Reply) -> Self {
        match reply {
            Reply::Failed(Error::Invalid) => Self::BadPartial,
            other => Self::Store(other),
        }
    }

    /// The refusal in words, for the shell's answer.
    pub fn why(&self) -> String {
        match self {
            Self::BadPartial => String::from(
                "the downloaded file changed on disk after its check: the store's hash of its copy is not the model's; the next models download checks it again and fetches what changed",
            ),
            Self::Store(reply) => format!("the store refused the model: {reply:?}"),
        }
    }
}

/// How `target` fits `machine`.
pub fn fit(target: &Signed, machine: &Machine<'_>) -> Fit {
    if machine
        .store
        .blobs
        .iter()
        .any(|b| b.sha256 == target.sha256)
    {
        return Fit::Stored;
    }
    let need_mib = kabi::model::guest_mib(target.length);
    if need_mib > machine.memory_mib {
        return Fit::Memory {
            need_mib,
            have_mib: machine.memory_mib,
        };
    }
    let need = disk_need(target.length, machine.held(&target.sha256));
    if need > machine.free_bytes {
        return Fit::Disk {
            need,
            free: machine.free_bytes,
        };
    }
    Fit::Fits
}

/// Whether `target` may be offered at all: a licence the project takes, and a
/// pinned source on one of `machine`'s hosts.
fn offerable(target: &Signed, machine: &Machine<'_>) -> bool {
    target.licence_ok()
        && Source::parse(&target.source).is_ok_and(|s| machine.hosts_of(&s).is_some())
}

/// The targets of the signed role this OS may download, largest first, each
/// with whether it fits; the largest that fits is the one recommended.
pub fn offers<'a>(signed: &'a [Signed], machine: &Machine<'_>) -> Vec<Offer<'a>> {
    let mut offers: Vec<Offer<'a>> = signed
        .iter()
        .filter(|t| offerable(t, machine))
        .map(|target| Offer {
            target,
            fit: fit(target, machine),
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

/// The ask as the shell's Allow / Deny menu puts it before the call, which
/// names the model: how large and from where, the two things question 6 asks
/// to be shown, short enough that the question fits the menu's 120 characters
/// ("Download 2.3 GiB from huggingface.co").
pub fn subject(target: &Signed, source: &Source) -> String {
    format!("Download {} from {}", size(target.length), source.host)
}

/// A download to run: what it fetches, from where, and where it lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub sha256: Sha256,
    pub length: u64,
    pub source: Source,
    /// The hosts it may reach: the source's, then its storage hosts.
    pub hosts: Vec<String>,
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
/// the role signs, under a licence the project takes, with a pinned source on
/// one of `machine`'s hosts, not stored yet, and it must fit `machine`.
pub fn plan(signed: &[Signed], machine: &Machine<'_>, id: &str) -> Result<Job, Refused> {
    let target = signed
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| Refused::NotSigned(String::from(id)))?;
    if !target.licence_ok() {
        return Err(Refused::Licence(target.licence.clone()));
    }
    let source = Source::parse(&target.source)?;
    let hosts = machine.hosts_of(&source).ok_or(Refused::Source(
        "the source's host is not one models come from",
    ))?;
    match fit(target, machine) {
        Fit::Fits => {}
        Fit::Stored => return Err(Refused::Stored),
        Fit::Memory { need_mib, have_mib } => return Err(Refused::Memory { need_mib, have_mib }),
        Fit::Disk { need, free } => return Err(Refused::Disk { need, free }),
    }
    let name = hex(&target.sha256);
    let in_store = |name: &str| format!("{MODELS_DIR}/{name}");
    Ok(Job {
        id: target.id.clone(),
        sha256: target.sha256,
        length: target.length,
        hosts: core::iter::once(hosts.source)
            .chain(hosts.storage.iter().copied())
            .map(String::from)
            .collect(),
        source,
        partial: in_store(&partial_name(&target.sha256)),
        path: in_store(&format!("{name}.gguf")),
        chunks: format!("{name}{CHUNKS_SUFFIX}"),
    })
}

impl Job {
    /// Holds a chunk list, as `kai-chunks 1` names its blob, to the role's
    /// target: the same SHA-256 and size, in [`MODEL_CHUNK`] chunks, or no
    /// byte is fetched.
    pub fn check_list(&self, blob: &Sha256, size: u64, chunk: u64) -> Result<(), Refused> {
        if *blob != self.sha256 {
            return Err(Refused::List("the chunk list is for another blob"));
        }
        if size != self.length {
            return Err(Refused::List("the chunk list's size is not the target's"));
        }
        if chunk != MODEL_CHUNK {
            return Err(Refused::List("the chunk list's chunks are not 2 MiB"));
        }
        Ok(())
    }

    /// Whether the download may reach `host` on `port`: the source's host or
    /// one its storage entries match ([`Hosts`]), on 443 only, since a pinned
    /// source is plain `https` with no port. `kainet` asks for every hop of a
    /// redirect.
    pub fn reaches(&self, host: &str, port: u16) -> bool {
        port == HTTPS_PORT && self.hosts.iter().any(|h| matches(h, host))
    }
}
