//! WebP: canvases, chunk sizes, ANMF fields and the lossless header (see `crafted`).

use super::crafted::*;
use super::samples;
use kaimedia::{Ceiling, Error, Format};

pub fn riff(body: &[u8]) -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + body.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(body);
    out
}

fn webp_chunk(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = name.to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// A VP8X chunk for a canvas of `w` x `h` (each at least 1) with `flags`.
fn vp8x(flags: u8, w: u32, h: u32) -> Vec<u8> {
    let mut body = vec![flags, 0, 0, 0];
    body.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
    body.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
    webp_chunk(b"VP8X", &body)
}

/// An animated file's canvas header: VP8X with the animation flag, and ANIM.
fn animated(w: u32, h: u32) -> Vec<u8> {
    let mut body = vp8x(0x02, w, h);
    body.extend(webp_chunk(b"ANIM", &[0; 6]));
    body
}

#[test]
fn a_webp_canvas_at_its_extremes_is_refused_before_its_buffer() {
    // Past the side ceiling, up to the 2^24 on a side that VP8X can say.
    for (w, h) in [(1 << 24, 1), (1, 1 << 24), (1_025, 1), (1_024, 1_025)] {
        refused_big(
            &format!("webp canvas {w}x{h}"),
            &samples::webp_claiming(w, h),
            &LIMITS,
            Ceiling::Side,
        );
    }
    // A canvas whose area image-webp cannot hold at all is refused by it, as a
    // bad file, before ours looks: no buffer either way.
    let bytes = samples::webp_claiming(1 << 24, 1 << 24);
    let (got, peak) = run_under("webp 2^24 x 2^24", &bytes, &LIMITS);
    assert!(
        matches!(got, Err(Error::Bad(Format::Webp, _))),
        "{:?}",
        got.map(dims)
    );
    assert!(
        peak <= SIZE_PEAK,
        "{peak} bytes allocated before the refusal"
    );
}

#[test]
fn a_webp_whose_chunks_lie_settles() {
    let good = samples::webp(&samples::rgba(8, 8, true), 8, 8, true);
    let settle = |name: &str, bytes: &[u8]| settles(name, bytes, Format::Webp, 8, 8);
    // RIFF's size, and the first chunk's, at their extremes.
    for size in [0u32, 4, 5, 0x7FFF_FFFF, 0xFFFF_FFFE, 0xFFFF_FFFF] {
        let mut bytes = good.clone();
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        settle(&format!("webp riff size {size}"), &bytes);
        let mut bytes = good.clone();
        bytes[16..20].copy_from_slice(&size.to_le_bytes());
        settle(&format!("webp chunk size {size}"), &bytes);
    }
    // Cut at every byte.
    for cut in 0..good.len() {
        cut_refused(&format!("webp cut at {cut}"), &good[..cut], Format::Webp);
    }
    // A header and no image, an unknown chunk first, an ANMF with no frame data.
    refused("webp vp8x only", &riff(&vp8x(0x10, 8, 8)), Format::Webp);
    let mut unknown = webp_chunk(b"JUNK", &[1, 2, 3]);
    unknown.extend(vp8x(0x10, 8, 8));
    refused("webp unknown first", &riff(&unknown), Format::Webp);
    let mut anmf = animated(8, 8);
    anmf.extend(webp_chunk(b"ANMF", &[0; 16]));
    refused("webp anmf without data", &riff(&anmf), Format::Webp);
}

#[test]
fn a_webp_frame_off_its_canvas_or_with_a_bad_lossless_header_settles() {
    let frames = [(samples::rgba(8, 8, true), 10)];
    let good = samples::webp_animated(8, 8, &frames);
    let anmf = good
        .windows(4)
        .position(|w| w == b"ANMF")
        .expect("the animated sample has an ANMF chunk");
    // ANMF's body: x, y, width - 1, height - 1 (3 bytes each), duration (3).
    for at in [anmf + 8, anmf + 11, anmf + 14, anmf + 17] {
        for value in [0u8, 1, 0x7F, 0xFF] {
            let mut bytes = good.clone();
            bytes[at..at + 3].fill(value);
            settles(
                &format!("webp anmf field {at} = {value}"),
                &bytes,
                Format::Webp,
                8,
                8,
            );
        }
    }
    // The lossless header (0x2F, then 14 bits of width - 1 and of height - 1)
    // at its extremes, and its transform and Huffman data replaced with fill.
    let lossless = samples::webp(&samples::rgba(8, 8, true), 8, 8, true);
    let vp8l = lossless
        .windows(4)
        .position(|w| w == b"VP8L")
        .expect("the lossless sample has a VP8L chunk");
    for header in [0u32, 0x3FFF, 0x3FFF << 14, 0x0FFF_FFFF, 0xFFFF_FFFF] {
        let mut bytes = lossless.clone();
        bytes[vp8l + 9..vp8l + 13].copy_from_slice(&header.to_le_bytes());
        // The canvas is the chunk's own here, so it may be any size in bounds.
        match body(&format!("webp vp8l header {header:#x}"), &bytes) {
            Ok(_) => {}
            Err(Error::Bad(format, _)) => assert_eq!(format, Format::Webp),
            Err(other) => panic!("webp vp8l header {header:#x}: {other:?}"),
        }
    }
    for fill in [0x00u8, 0x55, 0xFF] {
        let mut bytes = lossless.clone();
        bytes[vp8l + 13..].fill(fill);
        refused(&format!("webp vp8l data {fill:#x}"), &bytes, Format::Webp);
    }
}
