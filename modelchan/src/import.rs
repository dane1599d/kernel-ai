//! A model imported from an attached disk (ADR 0004 §8: SHA-256 is checked
//! once, at import). The disk is a GGUF file's bytes from its first sector,
//! rounded up to whole sectors with zeros, so its size names at most one target
//! of the signed role ([`comp_models::on_disk`]). The whole disk is read: SHA-256
//! and FNV-1a64 are taken over exactly the target's length, since the store
//! keys the blob by the one and `kai-modelhint 1` names it by both, and the
//! bytes after it must be zeros. A target under a licence the project does not
//! take is refused before any byte is read.

use crate::Sha256;
use comp_models::{GGUF_MAGIC, Refusal, Signed, check, on_disk};
use kabi::model::Fnv1a;
use sha2::{Digest, Sha256 as Hasher};

/// An import under way: what it reads and what it has hashed so far.
pub struct Importing {
    id: alloc::string::String,
    /// The target's length, the bytes hashed and stored.
    length: u64,
    /// The disk's size, the bytes read.
    disk: u64,
    /// Bytes of the disk given so far.
    read: u64,
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
        disk,
        read: 0,
        sha: Hasher::new(),
        fnv: Fnv1a::new(),
        head: [0; 4],
    })
}

impl Importing {
    /// The target's length: the bytes the blob holds.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// The disk's size: the bytes to read.
    pub fn disk(&self) -> u64 {
        self.disk
    }

    /// The disk's bytes given so far.
    pub fn read(&self) -> u64 {
        self.read
    }

    /// The target the disk's size names.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The disk's next bytes, in order, from its first; what is past its
    /// size is not taken. The first four must be GGUF's magic, the target's
    /// bytes are hashed, and the rest must be zeros. Returns the bytes of
    /// `bytes` that belong in the blob, which the caller stores.
    pub fn push<'b>(&mut self, bytes: &'b [u8]) -> Result<&'b [u8], Refusal> {
        let bytes = &bytes[..bytes.len().min((self.disk - self.read) as usize)];
        let keep = bytes
            .len()
            .min(self.length.saturating_sub(self.read) as usize);
        let (blob, tail) = bytes.split_at(keep);
        for (i, b) in blob.iter().enumerate().take(4) {
            if let Some(slot) = self.head.get_mut(self.read as usize + i) {
                *slot = *b;
            }
        }
        let before = self.read;
        self.read += bytes.len() as u64;
        if before < 4 && self.read.min(self.length) >= 4 && &self.head != GGUF_MAGIC {
            return Err(Refusal::NotGguf);
        }
        if tail.iter().any(|&b| b != 0) {
            return Err(Refusal::Trailing);
        }
        self.sha.update(blob);
        self.fnv.update(blob);
        Ok(blob)
    }

    /// Once the whole disk is in: the import, if its hash is the target's.
    pub fn finish(self, signed: &[Signed]) -> Result<Imported, Refusal> {
        if self.read != self.disk {
            return Err(Refusal::Short);
        }
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
