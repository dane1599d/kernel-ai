//! Hostile EDID blocks (ADR 0007 section 12): the display hands the kernel
//! these bytes. Every prefix, every field at every extreme value and seeded
//! mutations of a QEMU-like base block, each with its checksum made good again
//! so the bytes reach the readers: modes and a size or nothing, never a panic,
//! at most one mode per place a block can list one, sizes inside what the
//! fields can hold, and an allocation bounded by the block.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use gfxcore::edid::{modes, size_mm};

const BLOCK: usize = 128;
const MUTATIONS: usize = 4_000;
/// Four detailed timings, eight standard ones and sixteen established bits.
const MAX_MODES: usize = 4 + 8 + 16;
/// A detailed timing's 12-bit image size, the largest a block can give.
const MAX_MM: u32 = 0xFFF;
const ALLOC_FLOOR: usize = 4096;

/// A base block like QEMU's: preferred 1280x800, standard 1920x1080 and
/// 1600x1200, established 640x480 and 1024x768, 34 by 21 cm.
fn sample() -> Vec<u8> {
    let mut block = vec![0u8; BLOCK];
    block[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    block[0x12] = 1;
    block[0x13] = 4;
    block[0x15] = 34;
    block[0x16] = 21;
    block[0x23] = 0x20;
    block[0x24] = 0x08;
    for pair in block[0x26..0x36].chunks_mut(2) {
        pair.copy_from_slice(&[0x01, 0x01]);
    }
    block[0x26..0x28].copy_from_slice(&[(1920 / 8 - 31) as u8, 0xC0]);
    block[0x28..0x2A].copy_from_slice(&[(1600 / 8 - 31) as u8, 0x40]);
    let d = &mut block[0x36..0x48];
    d[0] = 0x01;
    d[1] = 0x1D;
    d[4] = 0x50;
    d[5] = 0x20;
    d[7] = 0x30;
    d[12] = 0x54;
    d[13] = 0x33;
    summed(block)
}

/// `block` with its checksum made good again, when it is long enough to have one.
fn summed(mut block: Vec<u8>) -> Vec<u8> {
    if block.len() >= BLOCK {
        let sum = block[..BLOCK - 1]
            .iter()
            .fold(0u8, |s, &b| s.wrapping_add(b));
        block[BLOCK - 1] = 0u8.wrapping_sub(sum);
    }
    block
}

fn check(edid: &[u8]) {
    let bound = edid.len() * 16 + ALLOC_FLOOR;
    let (found, peak) = hostile::peak_alloc(|| modes(edid));
    assert!(peak <= bound, "{peak} bytes for {} EDID bytes", edid.len());
    assert!(found.len() <= MAX_MODES, "{} modes", found.len());
    for (i, mode) in found.iter().enumerate() {
        assert!(mode.0 > 0 && mode.1 > 0, "an empty mode {mode:?}");
        assert!(!found[..i].contains(mode), "{mode:?} twice");
    }
    if let Some((width, height)) = size_mm(edid) {
        assert!(width > 0 && height > 0);
        assert!(width <= MAX_MM && height <= MAX_MM, "{width}x{height} mm");
    }
}

#[test]
fn the_sample_parses_as_built() {
    assert_eq!(modes(&sample()).first(), Some(&(1280, 800)));
    assert_eq!(size_mm(&sample()), Some((84, 51)));
}

#[test]
fn every_prefix_is_modes_or_nothing() {
    let block = sample();
    hostile::truncations(&block, |cut, prefix| {
        check(prefix);
        if cut < BLOCK {
            assert!(modes(prefix).is_empty(), "cut at {cut}");
            assert_eq!(size_mm(prefix), None, "cut at {cut}");
        }
    });
}

#[test]
fn every_field_at_every_extreme_with_a_good_checksum() {
    let block = sample();
    hostile::sweep_fields(&block, 0..BLOCK - 1, |_, mutated| {
        check(&summed(mutated.to_vec()));
    });
}

#[test]
fn seeded_mutations_with_good_checksums() {
    let block = sample();
    let mut rng = hostile::XorShift::new(0xED1D);
    let mut listed = 0;
    for _ in 0..MUTATIONS {
        let edid = summed(hostile::mutate(&mut rng, &block));
        check(&edid);
        listed += usize::from(!modes(&edid).is_empty());
    }
    // Most mutations keep the header and reach the readers.
    assert!(
        listed > MUTATIONS / 4,
        "{listed} of {MUTATIONS} listed modes"
    );
}

#[test]
fn every_byte_value_in_every_descriptor_and_timing() {
    let block = sample();
    for at in 0x12..0x7E {
        for value in [0u8, 1, 0x0F, 0x10, 0x7F, 0x80, 0xF0, 0xFF] {
            let mut edid = block.clone();
            edid[at] = value;
            check(&summed(edid));
        }
    }
}
