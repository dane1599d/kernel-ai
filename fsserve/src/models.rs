//! The model store's writes (ADR 0004 §8; P4 task 10): `updated`, the only
//! holder of the `models` volume root's `Dir` ([`MODELS_ROOT`], `dir=models`),
//! imports a signed model as a sealed blob and points `kai-modelhint 1` at the
//! one the next boot maps.
//!
//! - A blob is written inside the caller's own action, one at a time:
//!   [`Request::SealedBegin`], [`Request::SealedPush`] until every byte is in,
//!   [`Request::SealedFinish`], then Commit. Abort, or the caller's death,
//!   drops it with the action; a push or finish that fails aborts the action
//!   too; and Commit is refused while one is unfinished, so no half blob is
//!   ever durable.
//! - `fs` hashes the bytes as they come. A blob is kept only if their SHA-256
//!   is the one its name gives, `<64 hex>.gguf`, and their FNV-1a64 is kept in
//!   [`FNV_ATTR`]: nothing the caller says about the bytes is taken on trust.
//! - [`Request::ModelHint`] writes the hint, in `kai-data`'s first block, which
//!   kaifs leaves outside the filesystem, for a sealed blob of the store only.

use crate::{Call, Server, Target, error};
use fsproto::{Error, MAX_DATA, Reply};
use kabi::fs::{MODELS, MODELS_ROOT};
use kabi::handle::Rights;
use kabi::model::Fnv1a;
use kabi::modelhint::{FNV_ATTR, Hint};
use kaifs::{BLOCK, BlockDev, ROOT, SealedWriter};
use sha2::{Digest, Sha256};

/// A blob being written in the open action.
pub(crate) struct Sealing {
    writer: SealedWriter,
    /// The SHA-256 its name gives.
    named: [u8; 32],
    sha: Sha256,
    fnv: Fnv1a,
}

/// The SHA-256 a store blob's name, `<64 lower-case hex>.gguf`, gives.
pub fn blob_sha(name: &str) -> Option<[u8; 32]> {
    let hex = name.strip_suffix(".gguf")?.as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut sha = [0u8; 32];
    for (i, pair) in hex.chunks(2).enumerate() {
        sha[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(sha)
}

impl<D: BlockDev> Server<D> {
    /// Refuses a call not on the model store's `Dir` with WRITE.
    fn models_only(&self, call: &Call, target: &Target) -> Result<(), Error> {
        let store = call.badge == MODELS_ROOT && !target.file && target.object == ROOT;
        match store && call.rights.contains(Rights::WRITE) {
            true => Ok(()),
            false => Err(Error::Permission),
        }
    }

    pub(crate) fn sealed_begin(
        &mut self,
        call: &Call,
        target: &Target,
        name: &str,
        len: u64,
    ) -> Result<Reply, Error> {
        self.models_only(call, target)?;
        self.owned(call.caller)?;
        if self.sealing.is_some() {
            return Err(Error::Busy);
        }
        let named = blob_sha(name).ok_or(Error::Invalid)?;
        let fs = self.fs();
        fs.set_volume(MODELS.into()).map_err(error)?;
        let writer = fs.begin_sealed(ROOT, name, len).map_err(error)?;
        self.sealing = Some(Sealing {
            writer,
            named,
            sha: Sha256::new(),
            fnv: Fnv1a::new(),
        });
        Ok(Reply::Done)
    }

    pub(crate) fn sealed_push(
        &mut self,
        call: &Call,
        target: &Target,
        data: &[u8],
    ) -> Result<Reply, Error> {
        self.models_only(call, target)?;
        self.owned(call.caller)?;
        if data.len() > MAX_DATA {
            return Err(Error::Invalid);
        }
        let mut sealing = self.sealing.take().ok_or(Error::Invalid)?;
        let fs = self.fs();
        let pushed = fs
            .set_volume(MODELS.into())
            .and_then(|_| fs.push_sealed(&mut sealing.writer, data));
        if let Err(e) = pushed {
            return Err(self.failed(error(e)));
        }
        sealing.sha.update(data);
        sealing.fnv.update(data);
        self.sealing = Some(sealing);
        Ok(Reply::Done)
    }

    pub(crate) fn sealed_finish(&mut self, call: &Call, target: &Target) -> Result<Reply, Error> {
        self.models_only(call, target)?;
        self.owned(call.caller)?;
        let sealing = self.sealing.take().ok_or(Error::Invalid)?;
        let sha: [u8; 32] = sealing.sha.finalize().into();
        if sha != sealing.named {
            return Err(self.failed(Error::Invalid));
        }
        let fnv = sealing.fnv.finish();
        let fs = self.fs();
        let finished = fs
            .set_volume(MODELS.into())
            .and_then(|_| fs.finish_sealed(sealing.writer))
            .and_then(|ino| fs.set_xattr(ino, FNV_ATTR, &fnv.to_le_bytes()));
        match finished {
            Ok(()) => Ok(Reply::Done),
            Err(e) => Err(self.failed(error(e))),
        }
    }

    /// A blob that failed after its writer was taken aborts the whole action,
    /// so a later Commit cannot keep the half-written file it leaves.
    fn failed(&mut self, e: Error) -> Error {
        self.drop_changes();
        e
    }

    /// Writes `kai-modelhint 1` for sealed blob `name`, or no hint.
    pub(crate) fn model_hint(
        &mut self,
        call: &Call,
        target: &Target,
        name: Option<&str>,
    ) -> Result<Reply, Error> {
        self.models_only(call, target)?;
        match self.owner {
            Some(owner) if owner == call.caller => return Err(Error::Invalid),
            Some(_) => return Err(Error::Busy),
            None => {}
        }
        let mut block = [0u8; BLOCK];
        if let Some(name) = name {
            let sha256 = blob_sha(name).ok_or(Error::Invalid)?;
            let fs = self.fs();
            fs.set_volume(MODELS.into()).map_err(error)?;
            let entry = fs
                .lookup(ROOT, name)
                .map_err(error)?
                .ok_or(Error::Invalid)?;
            if !fs.is_sealed(entry.ino).map_err(error)? {
                return Err(Error::Invalid);
            }
            let len = fs.stat(entry.ino).map_err(error)?.size;
            let fnv = fs
                .get_xattr(entry.ino, FNV_ATTR)
                .map_err(error)?
                .and_then(|bytes| <[u8; 8]>::try_from(bytes.as_slice()).ok())
                .map(u64::from_le_bytes)
                .ok_or(Error::Invalid)?;
            let hint = Hint { len, fnv, sha256 };
            block[..kabi::modelhint::SIZE].copy_from_slice(&hint.encode());
        }
        let dev = &mut self.fs.io.dev;
        dev.write(0, &block).map_err(|_| Error::Io)?;
        dev.flush().map_err(|_| Error::Io)?;
        Ok(Reply::Done)
    }
}
