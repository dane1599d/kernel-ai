//! A namespace's blocks in the block layer's 512-byte sectors (G58). The
//! block layer moves 512-byte sectors; a namespace formatted with 4 KiB
//! blocks takes LBAs and counts in its own blocks, so the driver turns one
//! into the other, and refuses a request that is not whole blocks.

/// The block layer's sector.
pub const SECTOR: u64 = 512;

/// The shift from a namespace's blocks to 512-byte sectors, for the block
/// sizes the driver reads: 0 for 512 bytes, 3 for 4 KiB. None for others.
pub fn shift_for(block_bytes: u32) -> Option<u32> {
    match block_bytes {
        512 => Some(0),
        4096 => Some(3),
        _ => None,
    }
}

/// The LBA and block count for `bytes` from 512-byte sector `sector`, on a
/// namespace whose blocks are `1 << shift` sectors. None unless both are
/// whole blocks.
pub fn lba_range(sector: u64, bytes: u64, shift: u32) -> Option<(u64, u64)> {
    let block = SECTOR << shift;
    let whole = sector.is_multiple_of(1 << shift) && bytes.is_multiple_of(block);
    whole.then_some((sector >> shift, bytes / block))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_512_and_4096_byte_blocks_are_read() {
        assert_eq!(shift_for(512), Some(0));
        assert_eq!(shift_for(4096), Some(3));
        for other in [520, 1024, 2048, 8192, 65536] {
            assert_eq!(shift_for(other), None, "{other}");
        }
    }

    #[test]
    fn sectors_become_blocks_only_when_whole() {
        // 512-byte blocks: every sector is a block.
        assert_eq!(lba_range(7, 512, 0), Some((7, 1)));
        assert_eq!(lba_range(7, 100, 0), None);
        // 4 KiB blocks: sector 16 is LBA 2, and 8 KiB is two blocks.
        assert_eq!(lba_range(16, 8192, 3), Some((2, 2)));
        assert_eq!(lba_range(0, 4096, 3), Some((0, 1)));
        // A sector inside a block, or part of a block, is refused.
        assert_eq!(lba_range(17, 4096, 3), None);
        assert_eq!(lba_range(16, 512, 3), None);
        assert_eq!(lba_range(16, 4096 + 512, 3), None);
    }
}
