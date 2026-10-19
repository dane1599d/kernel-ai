//! DMA arithmetic: which buffers a device may reach without a bounce copy, and how
//! a disk image of any byte length is read in whole 512-byte sectors.

/// A disk sector, the unit of every virtio-blk transfer.
pub const SECTOR: u64 = 512;

/// The physical address of the buffer `virt..virt + len` when all of it lies in
/// the direct map, which maps physical `0..phys_end` at `hhdm` onwards. There a
/// virtual range is a physical range, so the device can use the buffer itself.
/// `None` for anything else, such as heap or stack memory, which must be bounced.
pub fn direct_map_phys(virt: u64, len: u64, hhdm: u64, phys_end: u64) -> Option<u64> {
    if len == 0 {
        return None;
    }
    let phys = virt.checked_sub(hhdm)?;
    let end = phys.checked_add(len)?;
    (end <= phys_end).then_some(phys)
}

/// Sectors that hold `len` bytes; the last one may be partly beyond the image.
pub fn sectors_for(len: u64) -> u64 {
    len.div_ceil(SECTOR)
}

/// Why a read was refused before it reached the driver, which would panic on a
/// length that is not a whole number of sectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// No sectors asked for.
    Empty,
    /// More than the staging buffer holds.
    TooLong,
    /// Past the last sector of the disk.
    PastEnd,
}

/// Checks a read of `sectors` sectors at `sector` against a disk of `capacity`
/// sectors and a staging buffer of `buffer` bytes; returns the bytes it moves,
/// always a non-zero multiple of [`SECTOR`].
pub fn check_read(sector: u64, sectors: u64, capacity: u64, buffer: u64) -> Result<u64, ReadError> {
    if sectors == 0 {
        return Err(ReadError::Empty);
    }
    let bytes = sectors.checked_mul(SECTOR).ok_or(ReadError::TooLong)?;
    if bytes > buffer {
        return Err(ReadError::TooLong);
    }
    match sector.checked_add(sectors) {
        Some(end) if end <= capacity => Ok(bytes),
        _ => Err(ReadError::PastEnd),
    }
}

/// One read of an image: `sectors` sectors from `sector`, landing at byte
/// `offset` of the image, of which the first `keep` bytes are the image's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub sector: u64,
    pub sectors: u64,
    pub offset: u64,
    pub keep: u64,
}

/// The reads that cover an image of `len` bytes, each at most `max_sectors`
/// sectors, in order. Only the last read may keep less than it moves.
pub fn chunks(len: u64, max_sectors: u64) -> impl Iterator<Item = Chunk> {
    let total = sectors_for(len);
    let step = max_sectors.max(1);
    let mut sector = 0;
    core::iter::from_fn(move || {
        if sector >= total {
            return None;
        }
        let sectors = step.min(total - sector);
        let offset = sector * SECTOR;
        let keep = (len - offset).min(sectors * SECTOR);
        let chunk = Chunk {
            sector,
            sectors,
            offset,
            keep,
        };
        sector += sectors;
        Some(chunk)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const HHDM: u64 = 0xFFFF_8000_0000_0000;
    const END: u64 = 512 * 1024 * 1024;

    #[test]
    fn a_buffer_inside_the_direct_map_needs_no_bounce() {
        assert_eq!(
            direct_map_phys(HHDM + 0x20_0000, 512 * 1024, HHDM, END),
            Some(0x20_0000)
        );
        // Ends exactly at the last tracked byte.
        assert_eq!(
            direct_map_phys(HHDM + END - 4096, 4096, HHDM, END),
            Some(END - 4096)
        );
    }

    #[test]
    fn heap_stack_and_straddling_buffers_are_bounced() {
        // The kernel heap and stacks sit far above the direct map.
        assert_eq!(direct_map_phys(0xFFFF_C000_0000_0000, 64, HHDM, END), None);
        assert_eq!(direct_map_phys(0xFFFF_C800_0001_0000, 16, HHDM, END), None);
        // Below the direct map, past its end, or running over it.
        assert_eq!(direct_map_phys(HHDM - 1, 16, HHDM, END), None);
        assert_eq!(direct_map_phys(HHDM + END, 1, HHDM, END), None);
        assert_eq!(direct_map_phys(HHDM + END - 4096, 4097, HHDM, END), None);
        assert_eq!(direct_map_phys(u64::MAX - 8, 64, 0, u64::MAX), None);
        assert_eq!(direct_map_phys(HHDM, 0, HHDM, END), None);
    }

    #[test]
    fn reads_are_checked_before_the_driver_sees_them() {
        let staging = 512 * 1024;
        assert_eq!(check_read(0, 1024, 5000, staging), Ok(staging));
        assert_eq!(check_read(4999, 1, 5000, staging), Ok(512));
        assert_eq!(check_read(0, 0, 5000, staging), Err(ReadError::Empty));
        assert_eq!(check_read(0, 1025, 5000, staging), Err(ReadError::TooLong));
        assert_eq!(check_read(4999, 2, 5000, staging), Err(ReadError::PastEnd));
        assert_eq!(
            check_read(u64::MAX, 1, u64::MAX, staging),
            Err(ReadError::PastEnd)
        );
        assert_eq!(
            check_read(0, u64::MAX, 5000, u64::MAX),
            Err(ReadError::TooLong)
        );
    }

    #[test]
    fn sector_counts_round_up() {
        assert_eq!(sectors_for(0), 0);
        assert_eq!(sectors_for(1), 1);
        assert_eq!(sectors_for(512), 1);
        assert_eq!(sectors_for(513), 2);
        // The Qwen3-0.6B Q8_0 file is not a whole number of sectors either.
        assert_eq!(sectors_for(639_446_688), 1_248_920);
    }

    #[test]
    fn chunks_cover_an_odd_image_once_and_keep_only_its_bytes() {
        let len = 3 * 1024 * 1024 + 777;
        let list: Vec<Chunk> = chunks(len, 1024).collect();
        // 512 KiB per read: six full reads, then the last two sectors.
        assert_eq!(list.len(), 7);
        assert_eq!(
            list[0],
            Chunk {
                sector: 0,
                sectors: 1024,
                offset: 0,
                keep: 512 * 1024
            }
        );
        let last = list[6];
        assert_eq!(last.sector, 6 * 1024);
        assert_eq!(last.sectors, 2);
        assert_eq!(last.keep, len - last.offset);
        assert!(last.keep < last.sectors * SECTOR);
        // Back to back, and together exactly the image.
        let mut next = 0;
        for chunk in &list {
            assert_eq!(chunk.sector, next);
            assert_eq!(chunk.offset, chunk.sector * SECTOR);
            assert!(chunk.sectors <= 1024);
            next += chunk.sectors;
        }
        assert_eq!(next, sectors_for(len));
        assert_eq!(list.iter().map(|c| c.keep).sum::<u64>(), len);
    }

    #[test]
    fn chunks_of_tiny_and_empty_images() {
        assert_eq!(chunks(0, 1024).count(), 0);
        let one: Vec<Chunk> = chunks(5, 1024).collect();
        assert_eq!(
            one,
            [Chunk {
                sector: 0,
                sectors: 1,
                offset: 0,
                keep: 5
            }]
        );
        // A zero step still makes progress.
        assert_eq!(chunks(2048, 0).count(), 4);
    }
}
