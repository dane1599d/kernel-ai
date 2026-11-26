//! Inputs built by hand to pass each format's magic and reach the deep
//! paths (G27's kaimedia half): zero and enormous sizes, chunks and segments
//! cut or lying about their length, JPEG Huffman and quantisation tables that
//! cannot be right, GIF LZW codes past the table, WebP frames off the canvas.
//! Each goes through `body`, which a fuzz target can call: it decodes under
//! small ceilings and checks that what decodes keeps them and that nothing
//! was allocated far past them. A refusal is fine; a panic, a hang or an
//! image past a ceiling is the bug.

use super::{harness, hostile, samples};
use kaimedia::{Error, Limits, decode, probe};

/// Ceilings small enough that a lie about a size costs little to refuse.
pub const LIMITS: Limits = Limits {
    max_input: 1 << 20,
    max_side: 1_024,
    max_pixels: 1 << 20,
    max_frames: 16,
    max_bytes: 8 << 20,
};

/// The most a decode may have held at once: the bytes decoded, a decoder's
/// scratch up to that again, and the input's own copies.
const PEAK: usize = 3 * (LIMITS.max_bytes as usize) + (2 << 20);

/// `bytes` under [`LIMITS`]: the result, with the ceilings checked.
pub fn body(name: &str, bytes: &[u8]) -> Result<kaimedia::Image, Error> {
    let (got, peak) = hostile::peak_alloc(|| decode(bytes, &LIMITS));
    assert!(peak <= PEAK, "{name}: {peak} bytes allocated at once");
    let probed = probe(bytes, &LIMITS);
    if let Ok(info) = &probed {
        assert!(
            info.width <= LIMITS.max_side && info.height <= LIMITS.max_side,
            "{name}"
        );
    }
    if let Ok(image) = &got {
        assert!(!image.frames.is_empty(), "{name}: no frames");
        assert!(image.frames.len() as u32 <= LIMITS.max_frames, "{name}");
        let canvas = image.width as usize * image.height as usize * 4;
        assert!(
            image.width > 0 && image.height > 0,
            "{name}: an empty canvas"
        );
        assert!(canvas as u64 <= LIMITS.max_bytes, "{name}");
        assert!(image.width.max(image.height) <= LIMITS.max_side, "{name}");
        let area = u64::from(image.width) * u64::from(image.height);
        assert!(area <= LIMITS.max_pixels, "{name}");
        let total: u64 = image.frames.iter().map(|f| f.rgba.len() as u64).sum();
        assert!(total <= LIMITS.max_bytes, "{name}");
        for frame in &image.frames {
            assert_eq!(frame.rgba.len(), canvas, "{name}");
        }
    }
    // The shared fuzz body holds for these bytes too.
    harness::media_decode(bytes);
    got
}

/// `body`, and the decode must be refused.
fn refused(name: &str, bytes: &[u8]) {
    if let Ok(image) = body(name, bytes) {
        panic!("{name}: decoded {}x{}", image.width, image.height);
    }
}

// --- PNG ------------------------------------------------------------------

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

fn png_header(w: u32, h: u32, depth: u8, colour: u8, interlace: u8) -> Vec<u8> {
    let mut out = PNG_MAGIC.to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[depth, colour, 0, 0, interlace]);
    samples::png_chunk(&mut out, b"IHDR", &ihdr);
    out
}

#[test]
fn a_png_of_no_size_is_refused() {
    for (w, h) in [(0, 0), (0, 8), (8, 0)] {
        let mut bytes = png_header(w, h, 8, 6, 0);
        samples::png_chunk(&mut bytes, b"IDAT", &[0x78, 0x9C, 0x63, 0, 0, 0, 1, 0, 1]);
        samples::png_chunk(&mut bytes, b"IEND", &[]);
        refused(&format!("png {w}x{h}"), &bytes);
    }
}

#[test]
fn a_png_header_that_cannot_be_right_is_refused_or_kept_within_the_ceilings() {
    // Every depth and colour type, valid together or not, and both interlaces.
    for depth in [0, 1, 2, 4, 8, 16, 17, 255] {
        for colour in [0, 1, 2, 3, 4, 5, 6, 7, 255] {
            for interlace in [0, 1, 2] {
                let mut bytes = png_header(8, 8, depth, colour, interlace);
                samples::png_chunk(&mut bytes, b"IDAT", &[0x78, 0x9C, 0x63, 0, 0, 0, 1, 0, 1]);
                samples::png_chunk(&mut bytes, b"IEND", &[]);
                let _ = body(&format!("png {depth}/{colour}/{interlace}"), &bytes);
            }
        }
    }
}

#[test]
fn a_png_chunk_that_lies_about_its_length_is_refused() {
    let good = samples::pngs().remove(0).1;
    // IHDR is at 8..33, the first chunk after it at 33.
    for length in [0u32, 1, 12, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF] {
        let mut bytes = good.clone();
        bytes[33..37].copy_from_slice(&length.to_be_bytes());
        let _ = body(&format!("png chunk length {length}"), &bytes);
    }
    // IHDR's own length.
    for length in [0u32, 12, 14, 0xFFFF_FFFF] {
        let mut bytes = good.clone();
        bytes[8..12].copy_from_slice(&length.to_be_bytes());
        refused(&format!("png IHDR length {length}"), &bytes);
    }
}

#[test]
fn a_png_cut_inside_each_chunk_is_refused() {
    let good = samples::pngs().remove(0).1;
    // Walk the chunks and cut inside the length, the name, the body and the CRC.
    let mut at = 8;
    while at + 12 <= good.len() {
        let length = u32::from_be_bytes(good[at..at + 4].try_into().unwrap()) as usize;
        for inside in [1, 3, 5, 8, 8 + length / 2, 8 + length, 10 + length] {
            let cut = at + inside;
            let name = format!("png cut at {cut}");
            // IEND closes a file whose pixels are all there: without it the
            // image is whole, so only its ceilings are checked.
            match &good[at + 4..at + 8] {
                b"IEND" => {
                    let _ = body(&name, &good[..cut.min(good.len())]);
                }
                _ if cut < good.len() => refused(&name, &good[..cut]),
                _ => {}
            }
        }
        at += 12 + length;
    }
}

#[test]
fn a_png_with_a_bad_palette_or_no_image_data_is_refused() {
    // Indexed colour with no PLTE, with an empty one and with 257 entries.
    for entries in [None, Some(0usize), Some(257), Some(1)] {
        let mut bytes = png_header(8, 8, 8, 3, 0);
        if let Some(n) = entries {
            samples::png_chunk(&mut bytes, b"PLTE", &vec![7u8; n * 3]);
        }
        samples::png_chunk(&mut bytes, b"IDAT", &[0x78, 0x9C, 0x63, 0, 0, 0, 1, 0, 1]);
        samples::png_chunk(&mut bytes, b"IEND", &[]);
        refused(&format!("png palette {entries:?}"), &bytes);
    }
    // No IDAT at all, then IDAT of nothing, then of garbage.
    for idat in [None, Some(vec![]), Some(vec![0xFF; 64])] {
        let mut bytes = png_header(8, 8, 8, 6, 0);
        if let Some(body) = &idat {
            samples::png_chunk(&mut bytes, b"IDAT", body);
        }
        samples::png_chunk(&mut bytes, b"IEND", &[]);
        refused("png without image data", &bytes);
    }
}

// --- JPEG -----------------------------------------------------------------

/// The offset of the first segment with marker `FF marker` in `bytes`.
fn segment(bytes: &[u8], marker: u8) -> usize {
    bytes
        .windows(2)
        .position(|m| m[0] == 0xFF && m[1] == marker)
        .unwrap_or_else(|| panic!("no FF{marker:02X} segment"))
}

#[test]
fn a_jpeg_of_no_size_is_refused() {
    for (w, h) in [(0, 0), (0, 8), (8, 0)] {
        refused(&format!("jpeg {w}x{h}"), &samples::jpeg_claiming(w, h));
    }
}

#[test]
fn a_jpeg_with_huffman_tables_that_cannot_be_right_is_refused_or_kept_in_bounds() {
    let good = samples::jpeg();
    let dht = segment(&good, 0xC4);
    let length = u16::from_be_bytes([good[dht + 2], good[dht + 3]]) as usize;
    // FF C4, length (2), class and id (1), 16 code counts, then the symbols.
    let counts = dht + 5..dht + 21;
    let body_end = dht + 2 + length;
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    for (what, fill) in [("all 0xFF", 0xFFu8), ("all zero", 0), ("all one", 1)] {
        let mut bytes = good.clone();
        bytes[counts.clone()].fill(fill);
        cases.push((what, bytes));
    }
    // Every code count at its extreme in turn.
    for at in 0..16 {
        for value in [0u8, 1, 0x7F, 0xFF] {
            let mut bytes = good.clone();
            bytes[dht + 5 + at] = value;
            cases.push(("one count", bytes));
        }
    }
    // The class and id nibbles.
    for byte in [0x00, 0x01, 0x04, 0x0F, 0x10, 0x11, 0x14, 0x1F, 0xF0, 0xFF] {
        let mut bytes = good.clone();
        bytes[dht + 4] = byte;
        cases.push(("class and id", bytes));
    }
    // The symbols all alike, and the table cut short.
    let mut bytes = good.clone();
    bytes[dht + 21..body_end].fill(0xFF);
    cases.push(("all symbols alike", bytes));
    let mut bytes = good.clone();
    bytes[dht + 2..dht + 4].copy_from_slice(&19u16.to_be_bytes());
    cases.push(("a table of 17 bytes", bytes));
    let mut bytes = good.clone();
    bytes[dht + 2..dht + 4].copy_from_slice(&0xFFFFu16.to_be_bytes());
    cases.push(("a table of 65533 bytes", bytes));
    for (name, bytes) in cases {
        let _ = body(&format!("jpeg huffman {name}"), &bytes);
    }
    // Every table gone: the scan has nothing to decode with.
    let mut bytes = good.clone();
    while let Some(at) = bytes.windows(2).position(|m| m[0] == 0xFF && m[1] == 0xC4) {
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        bytes.drain(at..at + 2 + length);
    }
    refused("jpeg without huffman tables", &bytes);
}

#[test]
fn a_jpeg_frame_header_that_cannot_be_right_is_refused_or_kept_in_bounds() {
    let good = samples::jpeg();
    let sof = segment(&good, 0xC0);
    // FF C0, length (2), precision (1), height (2), width (2), components (1),
    // then per component: id, sampling, quantisation table.
    for (what, at, values) in [
        ("precision", sof + 4, vec![0u8, 4, 8, 12, 16, 255]),
        ("components", sof + 9, vec![0, 1, 2, 4, 5, 255]),
        (
            "first sampling",
            sof + 11,
            vec![0x00, 0x01, 0x10, 0x44, 0x55, 0xFF],
        ),
        ("first table", sof + 12, vec![0, 3, 4, 255]),
    ] {
        for value in values {
            let mut bytes = good.clone();
            bytes[at] = value;
            let _ = body(&format!("jpeg {what} {value}"), &bytes);
        }
    }
    // The quantisation tables: all zero, all 0xFF, and a bad precision.
    let dqt = segment(&good, 0xDB);
    for (what, fill) in [("zero", 0u8), ("0xFF", 0xFF)] {
        let mut bytes = good.clone();
        let end = dqt + 2 + u16::from_be_bytes([bytes[dqt + 2], bytes[dqt + 3]]) as usize;
        bytes[dqt + 5..end].fill(fill);
        let _ = body(&format!("jpeg quantisation {what}"), &bytes);
    }
    // A scan header before any frame header.
    let mut bytes = good.clone();
    let sos = segment(&bytes, 0xDA);
    let scan: Vec<u8> = bytes.drain(sos..).collect();
    bytes.truncate(2);
    bytes.extend_from_slice(&scan);
    refused("jpeg scan before frame", &bytes);
}

#[test]
fn a_jpeg_cut_inside_each_segment_is_refused() {
    let good = samples::jpeg();
    let mut at = 2;
    while at + 4 <= good.len() && good[at] == 0xFF && good[at + 1] != 0xDA {
        let length = u16::from_be_bytes([good[at + 2], good[at + 3]]) as usize;
        for inside in [1, 2, 3, 4, 2 + length / 2, 1 + length] {
            let cut = at + inside;
            if cut < good.len() {
                refused(&format!("jpeg cut at {cut}"), &good[..cut]);
            }
        }
        at += 2 + length;
    }
}

// --- GIF ------------------------------------------------------------------

/// A GIF by hand: a canvas of `w` x `h` with a four-colour global table, then
/// `body` (blocks), then the trailer when `end`.
fn gif_with(w: u16, h: u16, body: &[u8], end: bool) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    // Global colour table of 4 colours, then background and aspect.
    out.extend_from_slice(&[0x81, 0, 0]);
    out.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    out.extend_from_slice(body);
    if end {
        out.push(0x3B);
    }
    out
}

/// An image descriptor at `left`,`top`, `w` x `h`, then LZW minimum code size
/// `min` and `data` as sub-blocks of at most 255 bytes.
fn gif_frame(left: u16, top: u16, w: u16, h: u16, flags: u8, min: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x2C];
    for v in [left, top, w, h] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.push(flags);
    out.push(min);
    for chunk in data.chunks(255) {
        out.push(chunk.len() as u8);
        out.extend_from_slice(chunk);
    }
    out.push(0);
    out
}

#[test]
fn a_gif_of_no_size_is_refused() {
    let frame = gif_frame(0, 0, 1, 1, 0, 2, &[0x44, 0x01]);
    for (w, h) in [(0, 0), (0, 4), (4, 0)] {
        refused(&format!("gif {w}x{h}"), &gif_with(w, h, &frame, true));
    }
}

#[test]
fn gif_lzw_codes_past_the_table_are_refused_or_kept_in_bounds() {
    // Every byte value fed as the whole stream, at every minimum code size,
    // so codes beyond the table, the clear code and the end code all turn up.
    for min in [0u8, 1, 2, 3, 7, 8, 9, 11, 12, 13, 255] {
        for byte in [0x00u8, 0x01, 0x04, 0x05, 0x0F, 0x7F, 0x80, 0xAA, 0xFF] {
            let data = vec![byte; 64];
            let frame = gif_frame(0, 0, 8, 8, 0, min, &data);
            let _ = body(
                &format!("gif lzw min {min} fill {byte:#x}"),
                &gif_with(8, 8, &frame, true),
            );
        }
    }
    // A code stream that grows the table to its 4096 limit and keeps going:
    // 2-bit codes are widened, so a long run of 0xFF reaches the top.
    let data = vec![0xFFu8; 4_000];
    let frame = gif_frame(0, 0, 8, 8, 0, 2, &data);
    let _ = body("gif lzw past 4096", &gif_with(8, 8, &frame, true));
    // The first code after a clear being one past the table.
    let frame = gif_frame(0, 0, 8, 8, 0, 2, &[0x0C, 0x1A, 0x2B, 0x3C]);
    let _ = body("gif lzw first code too high", &gif_with(8, 8, &frame, true));
    // Less data than the frame's pixels, and none at all.
    let frame = gif_frame(0, 0, 8, 8, 0, 2, &[0x44, 0x01]);
    let _ = body("gif lzw short", &gif_with(8, 8, &frame, true));
    let frame = gif_frame(0, 0, 8, 8, 0, 2, &[]);
    let _ = body("gif lzw empty", &gif_with(8, 8, &frame, true));
}

#[test]
fn a_gif_frame_off_the_canvas_or_without_a_palette_is_kept_in_bounds() {
    let data = [0x44u8, 0x01];
    for (left, top, w, h) in [
        (0, 0, 0, 0),
        (0, 0, 0, 5),
        (0xFFFF, 0xFFFF, 1, 1),
        (7, 7, 0xFFFF, 0xFFFF),
        (0, 0, 0xFFFF, 1),
        (0, 0, 1, 0xFFFF),
        (3, 3, 100, 100),
    ] {
        let frame = gif_frame(left, top, w, h, 0, 2, &data);
        let _ = body(
            &format!("gif frame {w}x{h} at {left},{top}"),
            &gif_with(8, 8, &frame, true),
        );
    }
    // A local table flag with no table following, and no table at all.
    let frame = gif_frame(0, 0, 2, 2, 0x87, 2, &data);
    let _ = body("gif local table missing", &gif_with(8, 8, &frame, true));
    let mut bytes = gif_with(8, 8, &gif_frame(0, 0, 2, 2, 0, 2, &data), true);
    bytes[10] = 0;
    let _ = body("gif no palette", &bytes);
}

#[test]
fn a_gif_cut_or_with_blocks_that_lie_is_refused_or_kept_in_bounds() {
    let good = gif_with(
        8,
        8,
        &gif_frame(0, 0, 8, 8, 0, 2, &[0x44, 0x01, 0x44, 0x01]),
        true,
    );
    for cut in 0..good.len() {
        let _ = body(&format!("gif cut at {cut}"), &good[..cut]);
    }
    // A sub-block longer than what is left, an extension with no end, and an
    // unknown block introducer.
    let long = gif_with(
        8,
        8,
        &[0x2C, 0, 0, 0, 0, 8, 0, 8, 0, 0, 2, 0xFF, 1, 2],
        false,
    );
    let _ = body("gif sub-block runs off the end", &long);
    let ext = gif_with(8, 8, &[0x21, 0xF9, 0xFF, 1, 2, 3], false);
    let _ = body("gif extension without an end", &ext);
    let unknown = gif_with(8, 8, &[0x99, 0x99, 0x99], false);
    let _ = body("gif unknown block", &unknown);
    // Many frames of nothing: the frame ceiling ends it.
    let mut many = Vec::new();
    for _ in 0..(LIMITS.max_frames * 4) {
        many.extend_from_slice(&gif_frame(0, 0, 1, 1, 0, 2, &[0x44, 0x01]));
    }
    refused("gif of too many frames", &gif_with(8, 8, &many, true));
}

// --- WebP -----------------------------------------------------------------

fn riff(body: &[u8]) -> Vec<u8> {
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

#[test]
fn a_webp_canvas_at_its_extremes_is_refused_before_its_buffer() {
    // A canvas of 2^24 on a side, the most VP8X can say.
    let mut body = vp8x(0x02, 1 << 24, 1 << 24);
    body.extend(webp_chunk(b"ANIM", &[0; 6]));
    let (got, peak) = hostile::peak_alloc(|| decode(&riff(&body), &LIMITS));
    assert!(got.is_err(), "a 2^24 canvas decoded");
    assert!(peak <= 1 << 20, "{peak} bytes allocated before the refusal");
    for (w, h) in [(1 << 24, 1), (1, 1 << 24), (1_025, 1), (1_024, 1_025)] {
        let mut body = vp8x(0x02, w, h);
        body.extend(webp_chunk(b"ANIM", &[0; 6]));
        refused(&format!("webp canvas {w}x{h}"), &riff(&body));
    }
}

#[test]
fn a_webp_whose_chunks_lie_is_refused_or_kept_in_bounds() {
    let good = samples::webp(&samples::rgba(8, 8, true), 8, 8, true);
    // RIFF's size, and the first chunk's, at their extremes.
    for size in [0u32, 4, 5, 0x7FFF_FFFF, 0xFFFF_FFFE, 0xFFFF_FFFF] {
        let mut bytes = good.clone();
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        let _ = body(&format!("webp riff size {size}"), &bytes);
        let mut bytes = good.clone();
        bytes[16..20].copy_from_slice(&size.to_le_bytes());
        let _ = body(&format!("webp chunk size {size}"), &bytes);
    }
    // Cut at every byte.
    for cut in 0..good.len() {
        let _ = body(&format!("webp cut at {cut}"), &good[..cut]);
    }
    // A header and no image, an unknown chunk first, an ANMF with no frame data.
    let _ = body("webp vp8x only", &riff(&vp8x(0x10, 8, 8)));
    let mut unknown = webp_chunk(b"JUNK", &[1, 2, 3]);
    unknown.extend(vp8x(0x10, 8, 8));
    let _ = body("webp unknown first", &riff(&unknown));
    let mut anmf = vp8x(0x02, 8, 8);
    anmf.extend(webp_chunk(b"ANIM", &[0; 6]));
    anmf.extend(webp_chunk(b"ANMF", &[0; 16]));
    let _ = body("webp anmf without data", &riff(&anmf));
}

#[test]
fn a_webp_frame_off_its_canvas_or_with_a_bad_lossless_header_is_in_bounds() {
    let frames = [(samples::rgba(8, 8, true), 10)];
    let good = samples::webp_animated(8, 8, &frames);
    // ANMF is the first chunk after VP8X (18..30) and ANIM (30..44): x, y,
    // width - 1, height - 1 (3 bytes each), duration (3) and flags.
    let anmf = good
        .windows(4)
        .position(|w| w == b"ANMF")
        .expect("an ANMF chunk");
    for at in [anmf + 8, anmf + 11, anmf + 14, anmf + 17] {
        for value in [0u8, 1, 0x7F, 0xFF] {
            for extent in [1usize, 3] {
                let mut bytes = good.clone();
                bytes[at..at + extent.min(3)].fill(value);
                let _ = body(&format!("webp anmf field {at} = {value}"), &bytes);
            }
        }
    }
    // The lossless header (0x2F, then 14 bits of width - 1 and of height - 1)
    // at its extremes, and its transform and Huffman data replaced with fill.
    let lossless = samples::webp(&samples::rgba(8, 8, true), 8, 8, true);
    if let Some(vp8l) = lossless.windows(4).position(|w| w == b"VP8L") {
        for header in [0u32, 0x3FFF, 0x3FFF << 14, 0x0FFF_FFFF, 0xFFFF_FFFF] {
            let mut bytes = lossless.clone();
            bytes[vp8l + 9..vp8l + 13].copy_from_slice(&header.to_le_bytes());
            let _ = body(&format!("webp vp8l header {header:#x}"), &bytes);
        }
        for fill in [0x00u8, 0x55, 0xFF] {
            let mut bytes = lossless.clone();
            let end = bytes.len();
            bytes[(vp8l + 13).min(end)..].fill(fill);
            let _ = body(&format!("webp vp8l data {fill:#x}"), &bytes);
        }
    }
}

// --- All four ---------------------------------------------------------------

#[test]
fn bytes_that_pass_only_the_magic_are_refused() {
    let magics: [(&str, Vec<u8>); 4] = [
        ("png", PNG_MAGIC.to_vec()),
        ("jpeg", vec![0xFF, 0xD8, 0xFF]),
        ("gif", b"GIF89a".to_vec()),
        ("webp", riff(&[])),
    ];
    for (name, magic) in magics {
        for tail in [
            vec![],
            vec![0u8; 64],
            vec![0xFFu8; 64],
            (0..=255u8).collect(),
        ] {
            let mut bytes = magic.clone();
            bytes.extend_from_slice(&tail);
            refused(&format!("{name} magic and {} bytes", tail.len()), &bytes);
        }
    }
}
