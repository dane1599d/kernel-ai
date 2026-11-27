//! A slot's writes in whole 4 KiB blocks (G58): [`write_blocks`] against a
//! slot that, as the kernel does on a 4Kn disk, refuses a write of part of a
//! block, one off a block, or one past `MAX_IO`.

use crate::blocks::{BLOCK, SECTOR, padded, write_blocks};
use crate::plan::Release;
use crate::slot::{Checkpoint, Mark, READ_BACK, SlotIo, SlotPartial};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use kabi::update::MAX_IO;
use kainet::download::{DownloadError, Partial};
use sha2::{Digest, Sha256};

/// Bytes no write has reached.
const UNWRITTEN: u8 = 0xEE;

/// A slot on a disk of 4 KiB blocks, written as `updated`'s `Slots` writes
/// one: through [`write_blocks`] with a `MAX_IO` buffer.
struct Disk4k {
    bytes: Vec<u8>,
    buf: Vec<u8>,
    /// Every write the disk took, as (byte offset, length).
    writes: Vec<(u64, usize)>,
}

impl Disk4k {
    fn new(len: usize) -> Self {
        Self {
            bytes: vec![UNWRITTEN; len],
            buf: vec![0; MAX_IO as usize],
            writes: Vec::new(),
        }
    }
}

/// SLOT_WRITE on a 4Kn disk: whole blocks from a block, at most `MAX_IO`.
fn slot_write(disk: &mut Disk4k, sector: u64, piece: &[u8]) -> Result<(), &'static str> {
    let at = sector * SECTOR;
    if !at.is_multiple_of(BLOCK) || !(piece.len() as u64).is_multiple_of(BLOCK) {
        return Err("block: a write of part of a block");
    }
    if piece.len() as u64 > MAX_IO {
        return Err("a write past MAX_IO");
    }
    let end = at as usize + piece.len();
    let to = disk
        .bytes
        .get_mut(at as usize..end)
        .ok_or("past the slot")?;
    to.copy_from_slice(piece);
    disk.writes.push((at, piece.len()));
    Ok(())
}

impl SlotIo for Disk4k {
    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), &'static str> {
        let mut buf = core::mem::take(&mut self.buf);
        let wrote = write_blocks(offset, bytes, &mut buf, |sector, piece| {
            slot_write(self, sector, piece)
        });
        self.buf = buf;
        wrote
    }
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let end = offset as usize + buf.len();
        buf.copy_from_slice(
            self.bytes
                .get(offset as usize..end)
                .ok_or("past the slot")?,
        );
        Ok(())
    }
    fn flush(&mut self) -> Result<(), &'static str> {
        Ok(())
    }
}

struct NoMark;
impl Checkpoint for NoMark {
    fn load(&mut self) -> Result<Option<Mark>, &'static str> {
        Ok(None)
    }
    fn save(&mut self, _: Option<Mark>) -> Result<(), &'static str> {
        Ok(())
    }
}

/// Whether the slot reads back, over `length` bytes, as `sha256`.
fn reads_back(slot: &mut Disk4k, length: usize, sha256: [u8; 32]) -> Result<(), DownloadError> {
    let release = Release {
        seq: 2,
        tag: String::from("v0.2.0"),
        length: length as u64,
        sha256,
    };
    SlotPartial::new(slot, &mut NoMark, &release).finish()
}

/// Lengths of 1, a block less a sector, whole blocks and one sector past a
/// full piece: each image lands whole, its last block is padded with zeros
/// and nothing past it is touched, and the read-back hashes only the image.
#[test]
fn an_image_is_written_in_whole_blocks_padded_with_zeros_and_hashed_to_its_length() {
    let k = 3 * BLOCK as usize;
    for len in [1, k - SECTOR as usize, k, MAX_IO as usize + SECTOR as usize] {
        let image: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(7) | 1).collect();
        let end = padded(len as u64) as usize;
        let mut slot = Disk4k::new(end + 2 * BLOCK as usize);
        slot.write(0, &image).unwrap();
        assert_eq!(slot.bytes[..len], image[..], "{len}");
        assert!(slot.bytes[len..end].iter().all(|&b| b == 0), "{len}");
        assert!(slot.bytes[end..].iter().all(|&b| b == UNWRITTEN), "{len}");
        let pieces: usize = slot.writes.iter().map(|&(_, n)| n).sum();
        assert_eq!(pieces, end, "{len}");
        let sha: [u8; 32] = Sha256::digest(&image).into();
        reads_back(&mut slot, len, sha).unwrap();
        if end != len {
            let padded_sha: [u8; 32] = Sha256::digest(&slot.bytes[..end]).into();
            let err = reads_back(&mut slot, len, padded_sha).unwrap_err();
            assert!(matches!(err, DownloadError::Store(READ_BACK)), "{len}");
        }
    }
}

/// A later chunk lands on its block; a write off a block is refused before
/// anything is written, and so is a buffer under a block.
#[test]
fn a_write_starts_on_a_block() {
    let mut slot = Disk4k::new(4 * BLOCK as usize);
    slot.write(BLOCK, &[0x5A; 100]).unwrap();
    assert_eq!(slot.writes, [(BLOCK, BLOCK as usize)]);
    assert_eq!(
        slot.write(SECTOR, &[0x5A; 100]),
        Err("a slot write starts on a block")
    );
    let mut small = [0u8; SECTOR as usize];
    let refused = write_blocks(0, &[1], &mut small, |_, _| Ok::<(), &str>(()));
    assert_eq!(refused, Err("a slot write's buffer is under a block"));
    assert_eq!(slot.writes.len(), 1);
    // An empty image writes nothing.
    slot.write(0, &[]).unwrap();
    assert_eq!(slot.writes.len(), 1);
}
