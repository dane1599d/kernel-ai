//! The model manager's model (P4 task 10): the models the store holds as
//! `/system/models/<sha256>.gguf` (ADR 0004 §8) and the ones the signed
//! `models` role names (ADR 0008 §6), each with its licence (ADR 0009), and what
//! the user's choices ask for. It reads what the OS hands it and answers each
//! choice with an [`Intent`]; the window carries the intent out through the
//! agent's rows (`model_use`, and the import from a disk), so every change is
//! logged and undoable as if the agent had made it. [`check`] is the rule the
//! importing service holds a blob to before it is stored. Nothing here touches
//! the OS itself.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub type Sha256 = [u8; 32];

/// The licences the project takes for models (decisions.md:84, ADR 0009).
pub const LICENCES: [&str; 2] = ["Apache-2.0", "MIT"];
/// The first bytes of every GGUF file.
pub const GGUF_MAGIC: &[u8; 4] = b"GGUF";
/// The fewest hex digits of a SHA-256 that [`Models::find`] takes for a model.
pub const MIN_PREFIX: usize = 8;

/// A model the signed `models` role names: one of its targets, as the service
/// that verified the role hands it over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    /// The target's name without `.gguf`, such as `qwen3-4b-2507`: the name
    /// `model_use` takes.
    pub id: String,
    /// What people read, such as `Qwen3-4B-Instruct-2507 Q4_K_M`.
    pub name: String,
    pub length: u64,
    pub sha256: Sha256,
    /// An SPDX id.
    pub licence: String,
    /// Where it comes from: the Hugging Face repository at its revision.
    pub source: String,
}

impl Signed {
    /// Whether it is under a licence the project takes.
    pub fn licence_ok(&self) -> bool {
        LICENCES.contains(&self.licence.as_str())
    }
}

/// A sealed blob in `/system/models`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stored {
    pub sha256: Sha256,
    pub length: u64,
}

/// What the store and `updated` say about which model is used (ADR 0008 §6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Store {
    pub blobs: Vec<Stored>,
    /// The model this boot runs, from `kai-modelhint 1`; none without one.
    pub running: Option<Sha256>,
    /// The model the next restart uses, when `model_use` chose another.
    pub next: Option<Sha256>,
    /// The running model is a candidate on probation: it commits once its
    /// header passes, `health` reports it loaded and the 5 smoke tasks pass.
    pub on_trial: bool,
    /// The candidate the last probation rolled back from, and why.
    pub rolled_back: Option<(Sha256, String)>,
}

/// An attached disk that may hold a model: a GGUF file's bytes from block 0,
/// as the `kai-model` disk holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    pub id: u32,
    pub label: String,
    /// Its size, in whole sectors: a file attached as a disk is rounded up to
    /// [`SECTOR`] bytes, so the disk does not say the file's own length.
    pub length: u64,
}

/// A disk's sector, in bytes.
pub const SECTOR: u64 = 512;

/// Why an import was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It does not start with GGUF's magic.
    NotGguf,
    /// No target of the role has its length and SHA-256.
    NotSigned,
    /// It has the length of the role's target `id` but not its SHA-256.
    Hash { id: String },
    /// The role's target is under a licence the project does not take.
    Licence(String),
    /// The disk is the size of two of the role's targets, `.0` and `.1`, so
    /// which one it holds is not known before it is read.
    TwoSizes(String, String),
}

impl Refusal {
    pub fn why(&self) -> String {
        match self {
            Refusal::NotGguf => String::from("it is not a GGUF file"),
            Refusal::NotSigned => String::from("the signed models role names no model like it"),
            Refusal::Hash { id } => {
                format!("it is the size of {id}, but its SHA-256 is not the one the role signs")
            }
            Refusal::TwoSizes(a, b) => format!("it is the size of both {a} and {b}"),
            Refusal::Licence(l) => {
                format!("it is under {l}; the project takes Apache-2.0 or MIT models only")
            }
        }
    }
}

/// The blob of `length` bytes and hash `sha256` as one of the role's targets,
/// or why it may not be stored: the rule an import is held to (ADR 0004 §8:
/// SHA-256 is checked once, at import).
pub fn check<'a>(
    signed: &'a [Signed],
    length: u64,
    sha256: &Sha256,
) -> Result<&'a Signed, Refusal> {
    match signed
        .iter()
        .find(|s| s.length == length && s.sha256 == *sha256)
    {
        Some(s) if s.licence_ok() => Ok(s),
        Some(s) => Err(Refusal::Licence(s.licence.clone())),
        None => match signed.iter().find(|s| s.length == length) {
            Some(s) => Err(Refusal::Hash { id: s.id.clone() }),
            None => Err(Refusal::NotSigned),
        },
    }
}

/// The one target of the role a disk of `disk` bytes could hold, from its
/// first byte: one whose length rounds up to the disk's whole sectors. The
/// import then reads exactly that many bytes and holds them to [`check`].
pub fn on_disk(signed: &[Signed], disk: u64) -> Result<&Signed, Refusal> {
    let fits = |s: &&Signed| s.length <= disk && disk - s.length < SECTOR;
    let mut targets = signed.iter().filter(fits);
    match (targets.next(), targets.next()) {
        (Some(one), None) => Ok(one),
        (Some(a), Some(b)) => Err(Refusal::TwoSizes(a.id.clone(), b.id.clone())),
        (None, _) => Err(Refusal::NotSigned),
    }
}

/// Whether a disk of `disk` bytes could hold one of the role's targets: the
/// check before a disk is read at all.
pub fn could_be(signed: &[Signed], disk: u64) -> bool {
    on_disk(signed, disk).is_ok()
}

/// An import, as the window shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Import {
    /// Disk `disk` is being copied and hashed: `done` of `length` bytes.
    Copying {
        disk: u32,
        done: u64,
        length: u64,
    },
    /// It matched the role and is in the store.
    Stored {
        disk: u32,
        sha256: Sha256,
    },
    Refused {
        disk: u32,
        why: Refusal,
    },
}

/// What the model manager asks the OS to do for a choice. Each is one of the
/// agent's rows, so it is logged and undoable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `model_use`: the stored model the next restart uses, on probation.
    Use(Sha256),
    /// Import disk `id`'s model into the store, held to [`check`]; undone by
    /// deleting the new blob.
    Import(u32),
}

/// Where a model stands, in the order the list shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// This boot runs it.
    Running,
    /// The next restart uses it.
    Next,
    Stored,
    /// The role signs it and the store does not hold it.
    Available,
}

/// One model in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row<'a> {
    pub sha256: Sha256,
    pub length: u64,
    /// Its target in the role, if the role names it.
    pub signed: Option<&'a Signed>,
    pub state: State,
}

impl Row<'_> {
    /// Its name in the role, or its hash's first 12 hex digits.
    pub fn name(&self) -> String {
        match self.signed {
            Some(s) => s.name.clone(),
            None => format!("sha256 {}", &hex(&self.sha256)[..12]),
        }
    }

    pub fn licence(&self) -> &str {
        self.signed
            .map_or("not in the signed role", |s| s.licence.as_str())
    }
}

/// Everything the window shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Models {
    pub signed: Vec<Signed>,
    pub store: Store,
    pub disks: Vec<Disk>,
    /// The import running, or the last one's result.
    pub import: Option<Import>,
}

impl Models {
    /// Every stored model and every model the role signs, each once: by
    /// [`State`], then by name.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows: Vec<Row<'_>> = Vec::new();
        let mut add = |sha256: Sha256, length: u64, stored: bool| {
            if rows.iter().any(|r| r.sha256 == sha256) {
                return;
            }
            let signed = self
                .signed
                .iter()
                .find(|s| s.sha256 == sha256 && s.length == length);
            let state = if !stored {
                State::Available
            } else if self.store.running == Some(sha256) {
                State::Running
            } else if self.store.next == Some(sha256) {
                State::Next
            } else {
                State::Stored
            };
            rows.push(Row {
                sha256,
                length,
                signed,
                state,
            });
        };
        for b in &self.store.blobs {
            add(b.sha256, b.length, true);
        }
        for s in &self.signed {
            add(s.sha256, s.length, false);
        }
        rows.sort_by_cached_key(|r| (r.state, r.name()));
        rows
    }

    pub fn row(&self, sha256: &Sha256) -> Option<Row<'_>> {
        self.rows().into_iter().find(|r| r.sha256 == *sha256)
    }

    /// The model `name` stands for: a target's id or name (in any case), or at
    /// least [`MIN_PREFIX`] hex digits that begin one model's SHA-256 only.
    pub fn find(&self, name: &str) -> Option<Row<'_>> {
        let rows = self.rows();
        let named = |r: &&Row<'_>| {
            r.signed.is_some_and(|s| {
                s.id.eq_ignore_ascii_case(name) || s.name.eq_ignore_ascii_case(name)
            })
        };
        if let Some(r) = rows.iter().find(named) {
            return Some(*r);
        }
        let prefix = name.to_ascii_lowercase();
        let ok = prefix.len() >= MIN_PREFIX && prefix.bytes().all(|b| b.is_ascii_hexdigit());
        let mut hits = rows
            .iter()
            .filter(|r| ok && hex(&r.sha256).starts_with(&prefix));
        match (hits.next(), hits.next()) {
            (Some(r), None) => Some(*r),
            _ => None,
        }
    }

    /// The model the next restart uses.
    pub fn next(&self) -> Option<Sha256> {
        self.store.next.or(self.store.running)
    }

    /// Using `sha256` from the next restart, if the store holds it, the role
    /// does not put it under a licence the project refuses, and the next
    /// restart would not use it anyway.
    pub fn use_model(&self, sha256: &Sha256) -> Option<Intent> {
        let row = self.row(sha256)?;
        let allowed = row.state != State::Available && row.signed.is_none_or(Signed::licence_ok);
        (allowed && self.next() != Some(*sha256)).then_some(Intent::Use(*sha256))
    }

    /// Importing disk `id`, if it is attached, is the size of a model the role
    /// signs, and no other import is running.
    pub fn import(&self, id: u32) -> Option<Intent> {
        let disk = self.disks.iter().find(|d| d.id == id)?;
        let busy = matches!(self.import, Some(Import::Copying { .. }));
        (!busy && could_be(&self.signed, disk.length)).then_some(Intent::Import(id))
    }

    /// Applies an intent the OS carried out, so the window shows it.
    pub fn applied(&mut self, intent: &Intent) {
        match intent {
            Intent::Use(sha256) => {
                self.store.next = (self.store.running != Some(*sha256)).then_some(*sha256);
            }
            Intent::Import(id) => {
                // The bytes the import reads: the target's, not the disk's.
                let length = self
                    .disks
                    .iter()
                    .find(|d| d.id == *id)
                    .and_then(|d| on_disk(&self.signed, d.length).ok())
                    .map_or(0, |s| s.length);
                self.import = Some(Import::Copying {
                    disk: *id,
                    done: 0,
                    length,
                });
            }
        }
    }

    /// How far the running import has got.
    pub fn progress(&mut self, done: u64) {
        if let Some(Import::Copying {
            done: at, length, ..
        }) = &mut self.import
        {
            *at = done.min(*length);
        }
    }

    /// The running import's end: the blob it stored, or why it was refused.
    pub fn imported(&mut self, result: Result<Stored, Refusal>) {
        let Some(Import::Copying { disk, .. }) = self.import else {
            return;
        };
        self.import = Some(match result {
            Ok(blob) => {
                if !self.store.blobs.contains(&blob) {
                    self.store.blobs.push(blob);
                }
                Import::Stored {
                    disk,
                    sha256: blob.sha256,
                }
            }
            Err(why) => Import::Refused { disk, why },
        });
    }
}

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `bytes` for people: `512 bytes`, `1.5 KiB`, `2.3 GiB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KiB", "MiB", "GiB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut unit = 1024u128;
    let mut at = 0;
    while at + 1 < UNITS.len() && u128::from(bytes) >= unit * 1024 {
        unit *= 1024;
        at += 1;
    }
    let tenths = u128::from(bytes) * 10 / unit;
    format!("{}.{} {}", tenths / 10, tenths % 10, UNITS[at])
}
