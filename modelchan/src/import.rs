//! A model imported from an attached disk (ADR 0004 §8: SHA-256 is checked
//! once, at import). The disk is a GGUF file's bytes from its first sector,
//! rounded up to whole sectors, so its size names at most one target of the
//! signed role ([`comp_models::on_disk`]); exactly that target's length is read,
//! and SHA-256 and FNV-1a64 are taken on the way: the store keys the blob by
//! the one, and `kai-modelhint 1` names it by both. A target under a licence
//! the project does not take is refused before any byte is read.

use crate::Sha256;
use comp_models::{GGUF_MAGIC, Refusal, Signed, check, on_disk};
use kabi::model::Fnv1a;
use sha2::{Digest, Sha256 as Hasher};

/// An import under way: what it reads and what it has hashed so far.
pub struct Importing {
    id: alloc::string::String,
    length: u64,
    done: u64,
    sha: Hasher,
    fnv: Fnv1a,
    /// The first bytes, until there are enough to check the magic.
    head: [u8; 4],
}

/// What an import stored: the role's target it is, and its hashes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub id: alloc::string::String,
    pub length: u64,
    pub sha256: Sha256,
    pub fnv: u64,
}

/// An import from a disk of `disk` bytes, held to `signed`: the one target it
/// can hold, or why it is not read at all.
pub fn start(signed: &[Signed], disk: u64) -> Result<Importing, Refusal> {
    let target = on_disk(signed, disk)?;
    if !target.licence_ok() {
        return Err(Refusal::Licence(target.licence.clone()));
    }
    Ok(Importing {
        id: target.id.clone(),
        length: target.length,
        done: 0,
        sha: Hasher::new(),
        fnv: Fnv1a::new(),
        head: [0; 4],
    })
}

impl Importing {
    /// The bytes to read: the target's length.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// The bytes hashed so far.
    pub fn done(&self) -> u64 {
        self.done
    }

    /// The target the disk's size names.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The disk's next bytes, in order, from its first: at most what is left
    /// of [`Self::length`]; the caller cuts the last read. The first four must
    /// be GGUF's magic.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), Refusal> {
        let left = self.length - self.done;
        let bytes = &bytes[..bytes.len().min(left as usize)];
        for (i, b) in bytes.iter().take(4).enumerate() {
            if let Some(slot) = self.head.get_mut(self.done as usize + i) {
                *slot = *b;
            }
        }
        let before = self.done;
        self.done += bytes.len() as u64;
        if before < 4 && self.done >= 4 && &self.head != GGUF_MAGIC {
            return Err(Refusal::NotGguf);
        }
        self.sha.update(bytes);
        self.fnv.update(bytes);
        Ok(())
    }

    /// Once every byte is in: the import, if its hash is the target's. Bytes
    /// left unread cannot give the target's hash, so a short read is refused
    /// as a wrong one.
    pub fn finish(self, signed: &[Signed]) -> Result<Imported, Refusal> {
        if &self.head != GGUF_MAGIC {
            return Err(Refusal::NotGguf);
        }
        let sha256: Sha256 = self.sha.finalize().into();
        let target = check(signed, self.length, &sha256)?;
        Ok(Imported {
            id: target.id.clone(),
            length: self.length,
            sha256,
            fnv: self.fnv.finish(),
        })
    }
}
