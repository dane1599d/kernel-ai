//! Sealed files (ADR 0004 §8; P2 task 10): a model blob, written once and never
//! rewritten. Each of its data items is one SEALED record of up to 2 MiB on its
//! 2 MiB boundary, so each 2 MiB of the blob lies in one top-level buddy run,
//! and `fs` hands the kernel its blocks for `MODEL_FILL` with no filesystem
//! parser in ring 0.
//!
//! A file is sealed when its first data item names a SEALED record. Then every
//! item does, each but the last holds 2 MiB, and none is missing. It reads as
//! any file does. Writes and truncation refuse it; rename, trash and remove
//! take it as any file. A copy made beside it by `restore` is an ordinary file.

use crate::Error;
use crate::dev::BlockDev;
use crate::fs::{Fs, record_of, sealed_of};
use crate::items::{K_DATA, MAX_FILE, SEALED_CHUNK};
use crate::tree::Key;
use alloc::vec;
use alloc::vec::Vec;

/// A sealed file being written in order, for bytes that arrive in many small
/// pieces, such as a blob streamed to `fs` a message at a time: [`Fs::begin_sealed`],
/// then [`Fs::push_sealed`] until every byte is in, then [`Fs::finish_sealed`].
/// It holds one 2 MiB piece. It belongs to the transaction it was begun in: the
/// caller commits after `finish_sealed`, or aborts and drops it.
pub struct SealedWriter {
    ino: u64,
    len: u64,
    /// Bytes written as SEALED records so far.
    at: u64,
    piece: Vec<u8>,
}

impl SealedWriter {
    /// The bytes given so far.
    pub fn done(&self) -> u64 {
        self.at + self.piece.len() as u64
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<D: BlockDev> Fs<D> {
    /// Makes `name` in `dir` a sealed file of `len` bytes: `fill` is given each
    /// 2 MiB piece in turn, the last shorter, and each becomes one SEALED record,
    /// so a blob of gigabytes needs only one piece in memory. The caller commits,
    /// or aborts if `fill` or a write fails. Returns the file's inode.
    pub fn write_sealed(
        &mut self,
        dir: u64,
        name: &str,
        len: u64,
        fill: &mut dyn FnMut(&mut [u8]) -> Result<(), Error>,
    ) -> Result<u64, Error> {
        let mut writer = self.begin_sealed(dir, name, len)?;
        let mut piece = vec![0; len.min(SEALED_CHUNK) as usize];
        while writer.done() < len {
            let n = (len - writer.done()).min(SEALED_CHUNK) as usize;
            fill(&mut piece[..n])?;
            self.push_sealed(&mut writer, &piece[..n])?;
        }
        self.finish_sealed(writer)
    }

    /// Starts `name` in `dir` as a sealed file of `len` bytes.
    pub fn begin_sealed(&mut self, dir: u64, name: &str, len: u64) -> Result<SealedWriter, Error> {
        self.writing()?;
        if len > MAX_FILE {
            return Err(Error::Invalid("a file over 256 TiB"));
        }
        let ino = self.create(dir, name)?;
        Ok(SealedWriter {
            ino,
            len,
            at: 0,
            piece: Vec::with_capacity(len.min(SEALED_CHUNK) as usize),
        })
    }

    /// The file's next `bytes`: each full 2 MiB piece, and the last, shorter
    /// one, becomes one SEALED record. Bytes past its length are refused.
    pub fn push_sealed(
        &mut self,
        writer: &mut SealedWriter,
        mut bytes: &[u8],
    ) -> Result<(), Error> {
        if bytes.len() as u64 > writer.len - writer.done() {
            return Err(Error::Invalid("more bytes than the sealed file's length"));
        }
        while !bytes.is_empty() {
            let want = (writer.len - writer.at).min(SEALED_CHUNK) as usize;
            let take = (want - writer.piece.len()).min(bytes.len());
            writer.piece.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if writer.piece.len() == want {
                self.put_sealed(Key::new(writer.ino, K_DATA, writer.at), &writer.piece)?;
                writer.at += want as u64;
                writer.piece.clear();
            }
        }
        Ok(())
    }

    /// Ends the file once every byte is in; its inode.
    pub fn finish_sealed(&mut self, writer: SealedWriter) -> Result<u64, Error> {
        if writer.at != writer.len {
            return Err(Error::Invalid("a sealed file is missing bytes"));
        }
        let mut inode = self.file(writer.ino)?;
        inode.size = writer.len;
        (inode.mtime, inode.ctime) = (self.clock, self.clock);
        self.put_inode(writer.ino, &inode)?;
        Ok(writer.ino)
    }

    /// Whether `ino` is a sealed file.
    pub fn is_sealed(&mut self, ino: u64) -> Result<bool, Error> {
        let raw = self.get_raw(&Key::new(ino, K_DATA, 0))?;
        Ok(raw.as_deref().is_some_and(sealed_of))
    }

    /// Refuses a sealed file, for the calls that would rewrite it.
    pub(crate) fn unsealed(&mut self, ino: u64) -> Result<(), Error> {
        match self.is_sealed(ino)? {
            true => Err(Error::Invalid("a sealed file is never rewritten")),
            false => Ok(()),
        }
    }

    /// A sealed file's blocks in file order, as runs of (kaifs block, blocks).
    /// kaifs's block 0 is `kai-data`'s, so `fs` passes them to `MODEL_FILL` as
    /// they are. Adjacent records join into one run. The last run ends with the
    /// last record's last block, which holds the tail and then zeros.
    pub fn sealed_extents(&mut self, ino: u64) -> Result<Vec<(u64, u64)>, Error> {
        let size = self.file(ino)?.size;
        if !self.is_sealed(ino)? {
            return Err(Error::Invalid("not a sealed file"));
        }
        let mut out: Vec<(u64, u64)> = Vec::new();
        let mut next = 0;
        for (at, raw) in self.chunks(ino, 0, u64::MAX)? {
            let (ptr, len) = record_of(&raw)
                .filter(|_| sealed_of(&raw) && at == next)
                .ok_or(Error::Corrupt("a sealed file's data"))?;
            next = at + len as u64;
            let blocks = u64::from(ptr.blocks);
            match out.last_mut() {
                Some((start, n)) if *start + *n == ptr.addr => *n += blocks,
                _ => out.push((ptr.addr, blocks)),
            }
        }
        if next != size {
            return Err(Error::Corrupt("a sealed file shorter than its size"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{Cache, check};
    use crate::fs::MODELS;
    use crate::items::{CHUNK, ROOT};
    use crate::ns_check::check_namespace;
    use crate::repair::repair;
    use crate::{Error, MemDisk};
    use alloc::string::String;

    /// A disk with a sealed blob of three pieces and an ordinary file.
    fn with_blob() -> (Fs<MemDisk>, u64, u64) {
        let mut fs = Fs::mkfs(MemDisk::new(8192), [6; 16], "sealed").unwrap();
        fs.set_volume(MODELS).unwrap();
        let blob = fs
            .write_sealed(ROOT, "m.gguf", 2 * SEALED_CHUNK + 4096, &mut |piece| {
                piece.fill(0x5E);
                Ok(())
            })
            .unwrap();
        let plain = fs.create(ROOT, "plain").unwrap();
        fs.write_file(plain, &[1; 300 << 10]).unwrap();
        fs.commit().unwrap();
        (fs, blob, plain)
    }

    fn checks(fs: &mut Fs<MemDisk>) -> Result<(), String> {
        let mut cache = Cache::default();
        check(fs, &mut cache)?;
        check_namespace(fs, &mut cache).map(|_| ())
    }

    /// Breaks `fs` with `damage`; check must say `why`, repair must fix it, and
    /// a second repair must find nothing.
    fn repaired(
        mut fs: Fs<MemDisk>,
        damage: impl FnOnce(&mut Fs<MemDisk>),
        why: &str,
    ) -> Fs<MemDisk> {
        damage(&mut fs);
        fs.commit().unwrap();
        let found = checks(&mut fs).unwrap_err();
        assert!(found.contains(why), "{found}");
        assert!(!repair(&mut fs).unwrap().is_empty());
        checks(&mut fs).unwrap();
        assert!(repair(&mut fs).unwrap().is_empty());
        fs
    }

    #[test]
    fn a_sealed_record_in_an_ordinary_file_is_dropped() {
        let (fs, _, plain) = with_blob();
        let mut fs = repaired(
            fs,
            |fs| {
                let key = Key::new(plain, K_DATA, 3 * CHUNK);
                fs.put_sealed(key, &[3; 4096]).unwrap();
            },
            "a SEALED record out of a sealed file's run",
        );
        assert!(!fs.is_sealed(plain).unwrap());
        assert_eq!(fs.read_file(plain).unwrap(), [1; 300 << 10]);
    }

    #[test]
    fn a_sealed_file_with_a_piece_gone_keeps_the_run_before_it() {
        let (fs, blob, _) = with_blob();
        let mut fs = repaired(
            fs,
            |fs| {
                fs.delete(&Key::new(blob, K_DATA, SEALED_CHUNK)).unwrap();
            },
            "",
        );
        assert!(fs.is_sealed(blob).unwrap());
        assert_eq!(fs.stat(blob).unwrap().size, SEALED_CHUNK);
        assert_eq!(
            fs.sealed_extents(blob).unwrap(),
            [(fs.sealed_extents(blob).unwrap()[0].0, 512)]
        );
    }

    #[test]
    fn a_blob_pushed_in_small_pieces_is_the_one_write_sealed_makes() {
        let len = 2 * SEALED_CHUNK + 4093;
        let byte = |at: u64| (at.wrapping_mul(2_654_435_761) >> 13) as u8;
        let bytes: Vec<u8> = (0..len).map(byte).collect();
        let mut one = Fs::mkfs(MemDisk::new(8192), [6; 16], "one").unwrap();
        one.set_volume(MODELS).unwrap();
        let mut at = 0;
        let whole = one
            .write_sealed(ROOT, "m.gguf", len, &mut |piece| {
                piece.copy_from_slice(&bytes[at..at + piece.len()]);
                at += piece.len();
                Ok(())
            })
            .unwrap();
        one.commit().unwrap();
        let mut many = Fs::mkfs(MemDisk::new(8192), [6; 16], "many").unwrap();
        many.set_volume(MODELS).unwrap();
        let mut writer = many.begin_sealed(ROOT, "m.gguf", len).unwrap();
        for piece in bytes.chunks(8000) {
            many.push_sealed(&mut writer, piece).unwrap();
        }
        assert_eq!(writer.done(), len);
        let pushed = many.finish_sealed(writer).unwrap();
        many.commit().unwrap();
        assert!(many.is_sealed(pushed).unwrap());
        assert_eq!(many.read_file(pushed).unwrap(), bytes);
        let blocks = |fs: &mut Fs<MemDisk>, ino| {
            let runs = fs.sealed_extents(ino).unwrap();
            runs.iter().map(|(_, n)| n).sum::<u64>()
        };
        assert_eq!(blocks(&mut many, pushed), blocks(&mut one, whole));
        checks(&mut many).unwrap();
    }

    #[test]
    fn a_pushed_blob_must_be_given_exactly_its_length() {
        let mut fs = Fs::mkfs(MemDisk::new(8192), [6; 16], "len").unwrap();
        fs.set_volume(MODELS).unwrap();
        let mut writer = fs.begin_sealed(ROOT, "m.gguf", 5000).unwrap();
        fs.push_sealed(&mut writer, &[1; 3000]).unwrap();
        assert_eq!(
            fs.push_sealed(&mut writer, &[1; 2001]),
            Err(Error::Invalid("more bytes than the sealed file's length"))
        );
        assert_eq!(writer.done(), 3000);
        assert_eq!(
            fs.finish_sealed(writer),
            Err(Error::Invalid("a sealed file is missing bytes"))
        );
        fs.abort().unwrap();
        let empty = fs.begin_sealed(ROOT, "e.gguf", 0).unwrap();
        assert!(empty.is_empty());
        let ino = fs.finish_sealed(empty).unwrap();
        assert_eq!(fs.stat(ino).unwrap().size, 0);
    }

    #[test]
    fn an_ordinary_record_in_a_sealed_file_is_dropped() {
        let (fs, blob, _) = with_blob();
        let fs = repaired(
            fs,
            |fs| {
                fs.put_record(Key::new(blob, K_DATA, SEALED_CHUNK), &[2; 4096])
                    .unwrap();
            },
            "a SEALED record out of a sealed file's run",
        );
        let mut fs = fs;
        assert_eq!(fs.stat(blob).unwrap().size, SEALED_CHUNK);
        assert_eq!(
            fs.write_at(blob, 0, b"x"),
            Err(Error::Invalid("a sealed file is never rewritten"))
        );
    }
}
