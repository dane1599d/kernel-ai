//! The display modes a base EDID block (VESA E-EDID 1.3/1.4) lists: its detailed
//! timings, standard timings and the common established timings. Refresh rates
//! are ignored; a virtual display shows any size it is given.

use alloc::vec::Vec;

/// A display mode, width by height in pixels.
pub type Mode = (u32, u32);

const BLOCK: usize = 128;
const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
const ESTABLISHED: usize = 0x23;
const STANDARD: usize = 0x26;
const DESCRIPTORS: usize = 0x36;
const DESCRIPTOR_LEN: usize = 18;

/// Established timing bits (bytes 0x23 and 0x24, most significant bit first).
/// Repeats at other refresh rates are dropped later.
const ESTABLISHED_MODES: [[Option<Mode>; 8]; 2] = [
    [
        Some((720, 400)),
        Some((720, 400)),
        Some((640, 480)),
        Some((640, 480)),
        Some((640, 480)),
        Some((640, 480)),
        Some((800, 600)),
        Some((800, 600)),
    ],
    [
        Some((800, 600)),
        Some((800, 600)),
        Some((832, 624)),
        None, // 1024x768 interlaced
        Some((1024, 768)),
        Some((1024, 768)),
        Some((1024, 768)),
        Some((1280, 1024)),
    ],
];

/// The modes `edid` lists, the preferred (first detailed) timing first and each
/// mode once. Empty when the base block is missing, has no header or fails its
/// checksum.
pub fn modes(edid: &[u8]) -> Vec<Mode> {
    let Some(block) = edid.get(..BLOCK) else {
        return Vec::new();
    };
    let checksum = block.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    if block[..8] != HEADER || checksum != 0 {
        return Vec::new();
    }
    let version = (block[0x12], block[0x13]);
    let mut found = Vec::new();
    let mut add = |mode: Mode| {
        if mode.0 > 0 && mode.1 > 0 && !found.contains(&mode) {
            found.push(mode);
        }
    };
    for descriptor in block[DESCRIPTORS..DESCRIPTORS + 4 * DESCRIPTOR_LEN].chunks(DESCRIPTOR_LEN) {
        if let Some(mode) = detailed(descriptor) {
            add(mode);
        }
    }
    for pair in block[STANDARD..STANDARD + 16].chunks(2) {
        if let Some(mode) = standard(pair[0], pair[1], version) {
            add(mode);
        }
    }
    for (byte, modes) in ESTABLISHED_MODES.iter().enumerate() {
        for (bit, mode) in modes.iter().enumerate() {
            if block[ESTABLISHED + byte] & (0x80 >> bit) != 0
                && let Some(mode) = *mode
            {
                add(mode);
            }
        }
    }
    found
}

/// A detailed timing descriptor's active area; None for the other descriptors
/// (pixel clock 0: monitor name, range limits and so on).
fn detailed(d: &[u8]) -> Option<Mode> {
    if d[0] == 0 && d[1] == 0 {
        return None;
    }
    let width = u32::from(d[2]) | (u32::from(d[4] & 0xF0) << 4);
    let height = u32::from(d[5]) | (u32::from(d[7] & 0xF0) << 4);
    Some((width, height))
}

/// A standard timing's mode; None for an unused slot.
fn standard(first: u8, second: u8, version: (u8, u8)) -> Option<Mode> {
    if (first, second) == (0x01, 0x01) || first == 0 {
        return None;
    }
    let width = (u32::from(first) + 31) * 8;
    let height = match second >> 6 {
        // 16:10 from EDID 1.3 on; 1:1 before.
        0 if version < (1, 3) => width,
        0 => width * 10 / 16,
        1 => width * 3 / 4,
        2 => width * 4 / 5,
        _ => width * 9 / 16,
    };
    Some((width, height))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::modes;
    use std::vec::Vec;

    /// A base block like QEMU's: preferred 1280x800, standard timings for
    /// 1920x1080 (16:9) and 1600x1200 (4:3), established 640x480 and 1024x768.
    pub(crate) fn sample() -> Vec<u8> {
        let mut block = std::vec![0u8; 128];
        block[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        block[0x12] = 1;
        block[0x13] = 4;
        block[0x23] = 0x20; // 640x480@60
        block[0x24] = 0x08; // 1024x768@60
        for pair in block[0x26..0x36].chunks_mut(2) {
            pair.copy_from_slice(&[0x01, 0x01]);
        }
        block[0x26..0x28].copy_from_slice(&[(1920 / 8 - 31) as u8, 0xC0]);
        block[0x28..0x2A].copy_from_slice(&[(1600 / 8 - 31) as u8, 0x40]);
        // Detailed timing: a pixel clock, then 1280 (0x500) by 800 (0x320).
        let d = &mut block[0x36..0x48];
        d[0] = 0x01;
        d[1] = 0x1D;
        d[2] = 0x00;
        d[4] = 0x50;
        d[5] = 0x20;
        d[7] = 0x30;
        // The second descriptor names the monitor.
        block[0x48 + 3] = 0xFC;
        let sum = block.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        block[127] = 0u8.wrapping_sub(sum);
        block
    }

    #[test]
    fn lists_detailed_standard_and_established_modes_once() {
        assert_eq!(
            modes(&sample()),
            [
                (1280, 800),
                (1920, 1080),
                (1600, 1200),
                (640, 480),
                (1024, 768)
            ]
        );
    }

    #[test]
    fn rejects_a_bad_checksum_header_or_short_block() {
        let mut bad = sample();
        bad[127] ^= 1;
        assert!(modes(&bad).is_empty());
        let mut headless = sample();
        headless[0] = 0x01;
        headless[127] = headless[127].wrapping_sub(1);
        assert!(modes(&headless).is_empty());
        assert!(modes(&sample()[..100]).is_empty());
    }

    #[test]
    fn old_edid_reads_aspect_zero_as_square() {
        let mut block = sample();
        block[0x13] = 2;
        block[0x2A..0x2C].copy_from_slice(&[(1024 / 8 - 31) as u8, 0x00]);
        let sum = block[..127].iter().fold(0u8, |s, &b| s.wrapping_add(b));
        block[127] = 0u8.wrapping_sub(sum);
        assert!(modes(&block).contains(&(1024, 1024)));
    }
}
