//! The golden images' file: a sparse dump. Magic `KAIGOLD1`, the block count
//! (u64), then each written block as its address (u64), a count of segments
//! (u16) and each segment as an offset (u16), a length (u16) and its bytes; the
//! zeros between segments are left out.

use kaifs::{BLOCK, MemDisk};
use std::rc::Rc;

const MAGIC: &[u8; 8] = b"KAIGOLD1";
/// Zeros this long or longer split a block's segments.
const GAP: usize = 16;

pub fn read_image(path: &str) -> MemDisk {
    let file = std::fs::read(path).expect("the golden images are committed beside this test");
    assert_eq!(&file[..8], MAGIC);
    let blocks = u64::from_le_bytes(file[8..16].try_into().unwrap());
    let mut disk = MemDisk::new(blocks);
    fn take<'a>(rest: &mut &'a [u8], n: usize) -> &'a [u8] {
        let (head, tail) = rest.split_at(n);
        *rest = tail;
        head
    }
    let u16_of = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]) as usize;
    let mut rest = &file[16..];
    while !rest.is_empty() {
        let addr = u64::from_le_bytes(take(&mut rest, 8).try_into().unwrap());
        let mut block = [0u8; BLOCK];
        for _ in 0..u16_of(take(&mut rest, 2)) {
            let off = u16_of(take(&mut rest, 2));
            let len = u16_of(take(&mut rest, 2));
            block[off..off + len].copy_from_slice(take(&mut rest, len));
        }
        disk.put(addr, Rc::new(block));
    }
    disk
}

/// A block's segments: its bytes, less runs of [`GAP`] or more zeros.
fn segments(block: &[u8; BLOCK]) -> Vec<(usize, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < BLOCK {
        if block[i] == 0 {
            i += 1;
            continue;
        }
        let start = i;
        let mut zeros = 0;
        while i < BLOCK && zeros < GAP {
            zeros = if block[i] == 0 { zeros + 1 } else { 0 };
            i += 1;
        }
        out.push((start, &block[start..i - zeros]));
    }
    out
}

/// Writes `disk`'s blocks to `path` as a sparse dump.
pub fn write_image(path: &str, disk: &MemDisk, blocks: u64) {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&blocks.to_le_bytes());
    for addr in 0..blocks {
        let Some(block) = disk.get(addr) else {
            continue;
        };
        let parts = segments(block);
        out.extend_from_slice(&addr.to_le_bytes());
        out.extend_from_slice(&(parts.len() as u16).to_le_bytes());
        for (off, bytes) in parts {
            out.extend_from_slice(&(off as u16).to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(bytes);
        }
    }
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, &out).unwrap();
    eprintln!("{path}: {} bytes", out.len());
}
