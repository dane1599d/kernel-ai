//! A slot written in whole blocks (G58). A disk of 4 KiB blocks takes only
//! whole ones, so every SLOT_WRITE `updated` makes of an image goes through
//! [`write_blocks`]: each piece starts on a [`BLOCK`], is at most one buffer,
//! and the last is padded with zeros to a whole block. The pad lies past the
//! image, in the slot's own free room, and is never counted: a read-back
//! hashes exactly the image's length.

use alloc::format;
use alloc::string::String;

pub use kabi::update::{BLOCK, SECTOR};

// Every online chunk starts on a block (`updated`'s `Slots` refuses one that
// does not), because every chunk size `kainet` takes is whole blocks.
const _: () = assert!(kainet::chunks::MIN_CHUNK.is_multiple_of(BLOCK));

/// The bytes a write of `len` takes on the disk, whole blocks; `None` when
/// that is past `u64` (a signed length can be anything).
pub const fn padded(len: u64) -> Option<u64> {
    len.checked_next_multiple_of(BLOCK)
}

/// Whether an image of `length` bytes may be written into a slot of `room`
/// bytes: not empty (an empty image would leave a staged slot holding old
/// bytes), and its whole blocks within the slot. The one room rule for the
/// update disk, the online plan and a copy of the running slot.
pub const fn fits(length: u64, room: u64) -> bool {
    match padded(length) {
        Some(padded) => length != 0 && padded <= room,
        None => false,
    }
}

/// The sectors a copy of the running slot writes: an image of `image`
/// sectors into a slot of `room`, by [`fits`], in whole blocks. Both counts
/// come from the disk, so their bytes are worked out checked: a count past
/// `u64` bytes is refused, not wrapped to one that fits.
pub fn copy_sectors(image: u64, room: u64) -> Result<u64, String> {
    let bytes = |sectors: u64, what: &str| {
        sectors
            .checked_mul(SECTOR)
            .ok_or_else(|| format!("{what} is {sectors} sectors, more bytes than u64 counts"))
    };
    let (image, room) = (bytes(image, "the image")?, bytes(room, "the slot")?);
    match padded(image) {
        Some(padded) if fits(image, room) => Ok(padded / SECTOR),
        _ => Err(format!(
            "the image is {}, more than the slot's {room}",
            shown(image)
        )),
    }
}

/// `len` for a refusal: its bytes, and the whole blocks it takes when that
/// is more.
pub fn shown(len: u64) -> String {
    match padded(len) {
        Some(padded) if padded == len => format!("{len} bytes"),
        Some(padded) => format!("{len} bytes ({padded} in whole 4 KiB blocks)"),
        None => format!("{len} bytes (more than whole 4 KiB blocks can count)"),
    }
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
        // At most the buffer's length, so this never overflows.
        let end = n.next_multiple_of(BLOCK as usize);
        buf[..n].copy_from_slice(&bytes[done..done + n]);
        buf[n..end].fill(0);
        write((offset + done as u64) / SECTOR, &buf[..end])?;
        done += n;
    }
    Ok(())
}
