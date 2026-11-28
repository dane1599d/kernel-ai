//! A slot written in whole blocks (G58). A disk of 4 KiB blocks takes only
//! whole ones, so every SLOT_WRITE `updated` makes of an image goes through
//! [`write_blocks`]: each piece starts on a [`BLOCK`], is at most one buffer,
//! and the last is padded with zeros to a whole block. The pad lies past the
//! image, in the slot's own free room, and is never counted: a read-back
//! hashes exactly the image's length.

pub use kabi::update::{BLOCK, SECTOR};

// Every online chunk starts on a block (`updated`'s `Slots` refuses one that
// does not), because every chunk size `kainet` takes is whole blocks.
const _: () = assert!(kainet::chunks::MIN_CHUNK.is_multiple_of(BLOCK));

/// The bytes a write of `len` takes on the disk: whole blocks.
pub const fn padded(len: u64) -> u64 {
    len.next_multiple_of(BLOCK)
}

/// Writes `bytes` from byte `offset` of a slot through `write(sector, piece)`,
/// `sector` in [`SECTOR`]s from the slot's start. Each piece is at most
/// `buf`'s whole blocks and starts on a block; the last is padded with zeros
/// to a whole block. `offset` must start a block.
pub fn write_blocks<E: From<&'static str>>(
    offset: u64,
    bytes: &[u8],
    buf: &mut [u8],
    mut write: impl FnMut(u64, &[u8]) -> Result<(), E>,
) -> Result<(), E> {
    if !offset.is_multiple_of(BLOCK) {
        return Err("a slot write starts on a block".into());
    }
    let step = buf.len() - buf.len() % BLOCK as usize;
    if step == 0 {
        return Err("a slot write's buffer is under a block".into());
    }
    let mut done = 0;
    while done < bytes.len() {
        let n = (bytes.len() - done).min(step);
        let end = padded(n as u64) as usize;
        buf[..n].copy_from_slice(&bytes[done..done + n]);
        buf[n..end].fill(0);
        write((offset + done as u64) / SECTOR, &buf[..end])?;
        done += n;
    }
    Ok(())
}
