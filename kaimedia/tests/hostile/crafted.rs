//! Inputs built by hand to pass each format's magic and reach the deep
//! paths (G27's kaimedia half): zero and enormous sizes, chunks and segments
//! cut or lying about their length, JPEG Huffman and quantisation tables that
//! cannot be right, GIF LZW codes past the table, WebP frames off the canvas.
//! Each goes through `body`, which a fuzz target can call: it decodes under
//! small ceilings and checks that what decodes keeps them and that nothing
//! was allocated far past them. A refusal is fine; a panic, a hang or an
//! image past a ceiling is the bug.

use super::{harness, hostile, samples};
use kaimedia::{Ceiling, Error, Format, Image, Limits, decode, probe};

/// Ceilings small enough that a lie about a size costs little to refuse.
pub const LIMITS: Limits = Limits {
    max_input: 1 << 20,
    max_side: 1_024,
    max_pixels: 1 << 20,
    max_frames: 16,
    max_bytes: 8 << 20,
};

/// The most a decode may have held at once: the bytes decoded, with the
/// decoder's scratch counted in them, and a MiB for the input's own copies.
const PEAK: usize = LIMITS.max_bytes as usize + (1 << 20);

/// The most a file refused for its claimed size may have allocated: a few
/// buffers and the decoders' fixed state (a PNG's inflate window), never the
/// claim, which is up to 4 GiB or more.
const SIZE_PEAK: usize = 256 << 10;

/// `bytes` under `limits`: the result and the peak allocation, with every
/// ceiling checked on what decoded.
fn run_under(name: &str, bytes: &[u8], limits: &Limits) -> (Result<Image, Error>, usize) {
    let (got, peak) = hostile::peak_alloc(|| decode(bytes, limits));
    if let Ok(info) = probe(bytes, limits) {
        assert!(
            info.width <= limits.max_side && info.height <= limits.max_side,
            "{name}"
        );
    }
    if let Ok(image) = &got {
        assert!(!image.frames.is_empty(), "{name}: no frames");
        assert!(image.frames.len() as u32 <= limits.max_frames, "{name}");
        assert!(
            image.width > 0 && image.height > 0,
            "{name}: an empty canvas"
        );
        assert!(image.width.max(image.height) <= limits.max_side, "{name}");
        let area = u64::from(image.width) * u64::from(image.height);
        assert!(area <= limits.max_pixels, "{name}");
        let canvas = image.width as usize * image.height as usize * 4;
        let total: u64 = image.frames.iter().map(|f| f.rgba.len() as u64).sum();
        assert!(total <= limits.max_bytes, "{name}");
        for frame in &image.frames {
            assert_eq!(frame.rgba.len(), canvas, "{name}");
        }
    }
    (got, peak)
}

/// `bytes` under [`LIMITS`]: the result, with the ceilings checked and the
/// peak held to [`PEAK`]. A fuzz target can call this with any bytes.
pub fn body(name: &str, bytes: &[u8]) -> Result<Image, Error> {
    let (got, peak) = run_under(name, bytes, &LIMITS);
    assert!(peak <= PEAK, "{name}: {peak} bytes allocated at once");
    // The shared fuzz body holds for these bytes too.
    harness::media_decode(bytes);
    got
}

/// Refused as a bad `format`, which is the decoder's own refusal of bytes that
/// break it: never a ceiling, and never a decode.
fn refused(name: &str, bytes: &[u8], format: Format) {
    match body(name, bytes) {
        Err(Error::Bad(got, _)) => assert_eq!(got, format, "{name}"),
        other => panic!(
            "{name}: wanted a bad {format:?} image, got {:?}",
            other.map(dims)
        ),
    }
}

/// Refused for claiming more than `ceiling` allows, with at most
/// [`SIZE_PEAK`] allocated on the way.
fn refused_big(name: &str, bytes: &[u8], limits: &Limits, ceiling: Ceiling) {
    let (got, peak) = run_under(name, bytes, limits);
    match got {
        Err(Error::TooBig(got)) => assert_eq!(got, ceiling, "{name}"),
        other => panic!("{name}: wanted {ceiling:?}, got {:?}", other.map(dims)),
    }
    assert!(
        peak <= SIZE_PEAK,
        "{name}: {peak} bytes allocated before the refusal"
    );
}

/// For bytes a lenient decoder may take: a decode must be the `w` x `h` the
/// case declares (the canvas never moves), and a refusal a bad `format`,
/// never a ceiling.
fn settles(name: &str, bytes: &[u8], format: Format, w: u32, h: u32) {
    match body(name, bytes) {
        Ok(image) => assert_eq!((image.width, image.height), (w, h), "{name}"),
        Err(Error::Bad(got, _)) => assert_eq!(got, format, "{name}"),
        Err(other) => panic!("{name}: {other:?}"),
    }
}

/// A file cut short: refused as a bad `format` once enough is left to name it,
/// and as of no format before that.
fn cut_refused(name: &str, bytes: &[u8], format: Format) {
    match kaimedia::sniff(bytes) {
        Some(_) => refused(name, bytes, format),
        None => assert_eq!(body(name, bytes).map(dims), Err(Error::Unknown), "{name}"),
    }
}

fn dims(image: Image) -> (u32, u32, usize) {
    (image.width, image.height, image.frames.len())
}

// --- PNG ------------------------------------------------------------------

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const IDAT: [u8; 9] = [0x78, 0x9C, 0x63, 0, 0, 0, 1, 0, 1];

fn png_header(w: u32, h: u32, depth: u8, colour: u8, interlace: u8) -> Vec<u8> {
    let mut out = PNG_MAGIC.to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[depth, colour, 0, 0, interlace]);
    samples::png_chunk(&mut out, b"IHDR", &ihdr);
    out
}

/// A PNG of `w` x `h` RGBA8 whose image data is too short for it.
fn png_short(w: u32, h: u32) -> Vec<u8> {
    let mut bytes = png_header(w, h, 8, 6, 0);
    samples::png_chunk(&mut bytes, b"IDAT", &IDAT);
    samples::png_chunk(&mut bytes, b"IEND", &[]);
    bytes
}

/// The first sample PNG, and where its first chunk after IHDR starts.
fn png_sample() -> (Vec<u8>, usize) {
    (samples::pngs().remove(0).1, 8 + 12 + 13)
}

#[test]
fn a_png_of_no_size_is_refused() {
    for (w, h) in [(0, 0), (0, 8), (8, 0)] {
        refused(&format!("png {w}x{h}"), &png_short(w, h), Format::Png);
    }
}

#[test]
fn a_png_header_that_cannot_be_right_is_refused() {
    // Every depth and colour type, valid together or not, and both interlaces:
    // the image data is too short for all of them.
    for depth in [0, 1, 2, 4, 8, 16, 17, 255] {
        for colour in [0, 1, 2, 3, 4, 5, 6, 7, 255] {
            for interlace in [0, 1, 2] {
                let mut bytes = png_header(8, 8, depth, colour, interlace);
                samples::png_chunk(&mut bytes, b"IDAT", &IDAT);
                samples::png_chunk(&mut bytes, b"IEND", &[]);
                refused(
                    &format!("png {depth}/{colour}/{interlace}"),
                    &bytes,
                    Format::Png,
                );
            }
        }
    }
}

#[test]
fn a_png_chunk_that_lies_about_its_length_is_refused() {
    let (good, first) = png_sample();
    for length in [0u32, 1, 12, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF] {
        let mut bytes = good.clone();
        bytes[first..first + 4].copy_from_slice(&length.to_be_bytes());
        refused(&format!("png chunk length {length}"), &bytes, Format::Png);
    }
    // IHDR's own length.
    for length in [0u32, 12, 14, 0xFFFF_FFFF] {
        let mut bytes = good.clone();
        bytes[8..12].copy_from_slice(&length.to_be_bytes());
        refused(&format!("png IHDR length {length}"), &bytes, Format::Png);
    }
}

#[test]
fn a_png_cut_inside_each_chunk_is_refused() {
    let (good, _) = png_sample();
    // Walk the chunks and cut inside the length, the name, the body and the CRC.
    let mut at = 8;
    while at + 12 <= good.len() {
        let length = u32::from_be_bytes(good[at..at + 4].try_into().unwrap()) as usize;
        let iend = &good[at + 4..at + 8] == b"IEND";
        for inside in [1, 3, 5, 8, 8 + length / 2, 8 + length, 10 + length] {
            let cut = at + inside;
            let name = format!("png cut at {cut}");
            match iend {
                // IEND closes a file whose pixels are all there: it decodes.
                true if cut < good.len() => settles(&name, &good[..cut], Format::Png, 64, 48),
                true => {}
                false => refused(&name, &good[..cut], Format::Png),
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
        samples::png_chunk(&mut bytes, b"IDAT", &IDAT);
        samples::png_chunk(&mut bytes, b"IEND", &[]);
        refused(&format!("png palette {entries:?}"), &bytes, Format::Png);
    }
    // No IDAT at all, then IDAT of nothing, then of garbage.
    for idat in [None, Some(vec![]), Some(vec![0xFF; 64])] {
        let mut bytes = png_header(8, 8, 8, 6, 0);
        if let Some(body) = &idat {
            samples::png_chunk(&mut bytes, b"IDAT", body);
        }
        samples::png_chunk(&mut bytes, b"IEND", &[]);
        refused("png without image data", &bytes, Format::Png);
    }
}

/// `count` chunks named `name` with no body, after IHDR of the sample PNG.
fn png_flooded(count: usize, name: &[u8; 4]) -> Vec<u8> {
    let (good, first) = png_sample();
    let mut bytes = good[..first].to_vec();
    for _ in 0..count {
        samples::png_chunk(&mut bytes, name, &[]);
    }
    bytes.extend_from_slice(&good[first..]);
    bytes
}

#[test]
fn a_flood_of_png_chunks_is_walked_in_bounds() {
    let (good, _) = png_sample();
    let canvas = decode(&good, &LIMITS).expect("the sample decodes");
    // As many empty chunks as the input ceiling lets in (12 bytes each).
    let count = (LIMITS.max_input - good.len()) / 12;
    let started = std::time::Instant::now();
    // Ancillary and private chunks are skipped: the image is the same.
    for name in [b"tEXt", b"prVt", b"zTXt"] {
        let flooded = png_flooded(count, name);
        assert!(flooded.len() <= LIMITS.max_input);
        let name = format!("png {count} x {}", String::from_utf8_lossy(name));
        match body(&name, &flooded) {
            Ok(image) => assert_eq!(image, canvas, "{name}"),
            Err(Error::Bad(format, _)) => assert_eq!(format, Format::Png, "{name}"),
            Err(other) => panic!("{name}: {other:?}"),
        }
    }
    // An unknown critical chunk is refused, however many.
    refused(
        "png critical flood",
        &png_flooded(count, b"ABCD"),
        Format::Png,
    );
    // One more chunk than the input ceiling lets in: refused before any parse.
    let over = png_flooded(count + 1_000, b"tEXt");
    assert!(over.len() > LIMITS.max_input);
    refused_big(
        "png flood past the input ceiling",
        &over,
        &LIMITS,
        Ceiling::Input,
    );
    assert!(
        started.elapsed().as_secs() < 30,
        "the floods took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_png_whose_image_data_is_in_one_byte_chunks_decodes_as_it_does_whole() {
    let (good, first) = png_sample();
    let whole = decode(&good, &LIMITS).expect("the sample decodes");
    // Every IDAT of the sample split into chunks of 1, 7 and 255 bytes.
    for piece in [1usize, 7, 255] {
        let mut bytes = good[..first].to_vec();
        let mut at = first;
        while at + 12 <= good.len() {
            let length = u32::from_be_bytes(good[at..at + 4].try_into().unwrap()) as usize;
            let name: [u8; 4] = good[at + 4..at + 8].try_into().unwrap();
            let data = &good[at + 8..at + 8 + length];
            if &name == b"IDAT" {
                for part in data.chunks(piece) {
                    samples::png_chunk(&mut bytes, b"IDAT", part);
                }
            } else {
                samples::png_chunk(&mut bytes, &name, data);
            }
            at += 12 + length;
        }
        let image = body(&format!("png idat in {piece}s"), &bytes).expect("split IDAT decodes");
        assert_eq!(image, whole, "IDAT in chunks of {piece}");
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

/// Where each segment before the scan starts, and how long it says it is.
fn jpeg_segments(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 2;
    while at + 4 <= bytes.len() && bytes[at] == 0xFF && bytes[at + 1] != 0xDA {
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        out.push((at, length));
        at += 2 + length;
    }
    out
}

fn jpeg_settles(name: &str, bytes: &[u8]) {
    settles(name, bytes, Format::Jpeg, samples::W, samples::H);
}

#[test]
fn a_jpeg_of_no_size_is_refused() {
    for (w, h) in [(0, 0), (0, 8), (8, 0)] {
        refused(
            &format!("jpeg {w}x{h}"),
            &samples::jpeg_claiming(w, h),
            Format::Jpeg,
        );
    }
}

#[test]
fn a_jpeg_with_huffman_tables_that_cannot_be_right_settles() {
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
    // The symbols all alike, and the table's length wrong.
    let mut bytes = good.clone();
    bytes[dht + 21..body_end].fill(0xFF);
    cases.push(("all symbols alike", bytes));
    for claimed in [19u16, 0xFFFF] {
        let mut bytes = good.clone();
        bytes[dht + 2..dht + 4].copy_from_slice(&claimed.to_be_bytes());
        cases.push(("a table of the wrong length", bytes));
    }
    for (name, bytes) in cases {
        jpeg_settles(&format!("jpeg huffman {name}"), &bytes);
    }
    // The all-0xFF and all-zero tables are broken, not just odd.
    for fill in [0xFFu8, 0] {
        let mut bytes = good.clone();
        bytes[counts.clone()].fill(fill);
        refused(&format!("jpeg counts all {fill:#x}"), &bytes, Format::Jpeg);
    }
    // Every table gone: the scan has nothing to decode with.
    let mut bytes = good.clone();
    while let Some(at) = bytes.windows(2).position(|m| m[0] == 0xFF && m[1] == 0xC4) {
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        bytes.drain(at..at + 2 + length);
    }
    refused("jpeg without huffman tables", &bytes, Format::Jpeg);
}

#[test]
fn a_jpeg_frame_header_that_cannot_be_right_settles() {
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
            jpeg_settles(&format!("jpeg {what} {value}"), &bytes);
        }
    }
    // Nothing but 1, 3 or 4 components, and 8 bits, is a baseline JPEG here.
    for (at, value) in [
        (sof + 9, 0u8),
        (sof + 9, 2),
        (sof + 9, 5),
        (sof + 4, 0),
        (sof + 4, 16),
    ] {
        let mut bytes = good.clone();
        bytes[at] = value;
        refused(&format!("jpeg byte {at} = {value}"), &bytes, Format::Jpeg);
    }
    // The quantisation tables: all zero and all 0xFF decode to a (wrong)
    // image of the declared size.
    let dqt = segment(&good, 0xDB);
    for (what, fill) in [("zero", 0u8), ("0xFF", 0xFF)] {
        let mut bytes = good.clone();
        let end = dqt + 2 + u16::from_be_bytes([bytes[dqt + 2], bytes[dqt + 3]]) as usize;
        bytes[dqt + 5..end].fill(fill);
        jpeg_settles(&format!("jpeg quantisation {what}"), &bytes);
    }
    // A scan header before any frame header.
    let mut bytes = good.clone();
    let sos = segment(&bytes, 0xDA);
    let scan: Vec<u8> = bytes.drain(sos..).collect();
    bytes.truncate(2);
    bytes.extend_from_slice(&scan);
    refused("jpeg scan before frame", &bytes, Format::Jpeg);
}

#[test]
fn a_jpeg_segment_that_lies_about_its_length_settles() {
    let good = samples::jpeg();
    let segments = jpeg_segments(&good);
    assert!(
        segments.len() >= 4,
        "the sample has {} segments",
        segments.len()
    );
    for (at, length) in segments {
        let marker = good[at + 1];
        for claimed in [
            0,
            1,
            2,
            3,
            length.saturating_sub(1),
            length + 1,
            length + 1_000,
            0xFFFF,
        ] {
            let mut bytes = good.clone();
            bytes[at + 2..at + 4].copy_from_slice(&(claimed as u16).to_be_bytes());
            jpeg_settles(&format!("jpeg FF{marker:02X} claims {claimed}"), &bytes);
        }
    }
    // The scan header's own length, and its component count.
    let sos = segment(&good, 0xDA);
    for claimed in [0u16, 1, 2, 3, 0xFFFF] {
        let mut bytes = good.clone();
        bytes[sos + 2..sos + 4].copy_from_slice(&claimed.to_be_bytes());
        jpeg_settles(&format!("jpeg SOS claims {claimed}"), &bytes);
    }
}

#[test]
fn a_jpeg_cut_inside_each_segment_is_refused() {
    let good = samples::jpeg();
    for (at, length) in jpeg_segments(&good) {
        for inside in [1, 2, 3, 4, 2 + length / 2, 1 + length] {
            let cut = at + inside;
            assert!(cut < good.len());
            refused(&format!("jpeg cut at {cut}"), &good[..cut], Format::Jpeg);
        }
    }
}

#[test]
fn a_jpeg_cut_inside_its_scan_data_settles() {
    let good = samples::jpeg();
    let sos = segment(&good, 0xDA);
    let scan = sos + 2 + u16::from_be_bytes([good[sos + 2], good[sos + 3]]) as usize;
    assert!(
        good.len() - scan > 100,
        "{} bytes of scan data",
        good.len() - scan
    );
    // Every cut from the scan header to the end: a decode is the declared
    // canvas, a refusal a bad JPEG, never a ceiling or a panic.
    for cut in sos..good.len() {
        jpeg_settles(&format!("jpeg cut in scan at {cut}"), &good[..cut]);
    }
    // Cut inside the scan header, it is refused.
    for cut in [sos + 2, sos + 4, scan - 1] {
        refused(
            &format!("jpeg cut inside the scan header at {cut}"),
            &good[..cut],
            Format::Jpeg,
        );
    }
    // With no scan data at all, or any part of it, zune pads the rest and the
    // file decodes: a flat or part-drawn image of the declared size. Every
    // other format refuses a cut file (see `images_cut_anywhere_...`), so this
    // is pinned here, and reported, rather than passed over: turning on zune's
    // strict mode would refuse it, at the price of any file it warns about.
    for cut in [scan, scan + 1, (scan + good.len()) / 2] {
        let image = body(&format!("jpeg cut in scan at {cut}"), &good[..cut]);
        assert!(image.is_ok(), "jpeg cut in scan at {cut}: {image:?}");
    }
}

// --- GIF ------------------------------------------------------------------

/// A GIF by hand: a canvas of `w` x `h`, `palette` as the global table (a
/// power of two entries from 2 to 256, or none), then `body` (blocks), then
/// the trailer when `end`.
fn gif_with(w: u16, h: u16, body: &[u8], end: bool) -> Vec<u8> {
    gif_palette(w, h, Some(2), body, end)
}

fn gif_palette(w: u16, h: u16, palette: Option<u8>, body: &[u8], end: bool) -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    // Flags: a table of 2^(size+1) colours; then background and aspect.
    match palette {
        Some(size) => {
            out.extend_from_slice(&[0x80 | size, 0, 0]);
            out.extend(
                std::iter::repeat_n([0u8, 255, 0, 0, 0, 255, 0, 0, 0].iter().copied(), 1)
                    .flatten()
                    .cycle()
                    .take(3 << (size + 1)),
            );
        }
        None => out.extend_from_slice(&[0, 0, 0]),
    }
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

/// An 8 x 8 GIF of one frame that fills it, made by the encoder.
fn gif_8x8() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, 8, 8, &[0, 0, 0, 255, 255, 255]).unwrap();
        let pixels: Vec<u8> = (0..64).map(|n| (n % 2) as u8).collect();
        encoder
            .write_frame(&gif::Frame::from_indexed_pixels(8, 8, pixels, None))
            .unwrap();
    }
    out
}

fn gif_settles(name: &str, bytes: &[u8]) {
    settles(name, bytes, Format::Gif, 8, 8);
}

#[test]
fn a_gif_of_no_size_is_refused() {
    let frame = gif_frame(0, 0, 1, 1, 0, 2, &[0x44, 0x01]);
    for (w, h) in [(0, 0), (0, 4), (4, 0)] {
        refused(
            &format!("gif {w}x{h}"),
            &gif_with(w, h, &frame, true),
            Format::Gif,
        );
    }
}

#[test]
fn gif_lzw_codes_past_the_table_settle() {
    // Every byte value fed as the whole stream, at every minimum code size,
    // so codes beyond the table, the clear code and the end code all turn up.
    let (mut decoded, mut refused_n) = (0, 0);
    for min in [0u8, 1, 2, 3, 7, 8, 9, 11, 12, 13, 255] {
        for byte in [0x00u8, 0x01, 0x04, 0x05, 0x0F, 0x7F, 0x80, 0xAA, 0xFF] {
            let frame = gif_frame(0, 0, 8, 8, 0, min, &[byte; 64]);
            let name = format!("gif lzw min {min} fill {byte:#x}");
            let bytes = gif_with(8, 8, &frame, true);
            gif_settles(&name, &bytes);
            match body(&name, &bytes) {
                Ok(_) => decoded += 1,
                Err(_) => refused_n += 1,
            }
            // A minimum code size past 11 is not LZW the format allows.
            if min > 11 {
                refused(&name, &bytes, Format::Gif);
            }
        }
    }
    // Both outcomes turn up, so the streams reach the decoder's table code.
    assert!(
        decoded > 0 && refused_n > 0,
        "{decoded} decoded, {refused_n} refused"
    );
    // A code stream that grows the table to its 4096 limit and keeps going.
    let frame = gif_frame(0, 0, 8, 8, 0, 2, &vec![0xFFu8; 4_000]);
    gif_settles("gif lzw past 4096", &gif_with(8, 8, &frame, true));
    // Less data than the frame's pixels, and none at all: refused.
    for (name, data) in [("short", &[0x44u8, 0x01][..]), ("empty", &[][..])] {
        let frame = gif_frame(0, 0, 8, 8, 0, 2, data);
        refused(
            &format!("gif lzw {name}"),
            &gif_with(8, 8, &frame, true),
            Format::Gif,
        );
    }
    // The first code after a clear being one past the table.
    let frame = gif_frame(0, 0, 8, 8, 0, 2, &[0x0C, 0x1A, 0x2B, 0x3C]);
    refused(
        "gif lzw first code too high",
        &gif_with(8, 8, &frame, true),
        Format::Gif,
    );
}

#[test]
fn a_gif_frame_off_the_canvas_or_without_a_palette_settles() {
    let data = [0x44u8, 0x01];
    // Frames that fit the ceilings and the data cannot fill, or sit off the canvas.
    for (left, top, w, h) in [
        (0, 0, 0, 0),
        (0, 0, 0, 5),
        (0xFFFF, 0xFFFF, 1, 1),
        (3, 3, 100, 100),
    ] {
        let frame = gif_frame(left, top, w, h, 0, 2, &data);
        gif_settles(
            &format!("gif frame {w}x{h} at {left},{top}"),
            &gif_with(8, 8, &frame, true),
        );
    }
    // A frame wider or taller than the side ceiling is refused by it, before
    // its pixels are read.
    for (left, top, w, h) in [(7, 7, 0xFFFF, 0xFFFF), (0, 0, 0xFFFF, 1), (0, 0, 1, 0xFFFF)] {
        let frame = gif_frame(left, top, w, h, 0, 2, &data);
        refused_big(
            &format!("gif frame {w}x{h} at {left},{top}"),
            &gif_with(8, 8, &frame, true),
            &LIMITS,
            Ceiling::Side,
        );
    }
    // A frame of the whole canvas that the data cannot fill is refused; a
    // sliver at the far corner draws nothing but is a frame all the same.
    let sliver = gif_frame(0xFFFF, 0xFFFF, 1, 1, 0, 2, &data);
    let image = body("gif sliver", &gif_with(8, 8, &sliver, true)).expect("a sliver decodes");
    assert_eq!((image.width, image.height, image.frames.len()), (8, 8, 1));
    // A local table flag with no table following, and no table at all.
    let frame = gif_frame(0, 0, 2, 2, 0x87, 2, &data);
    gif_settles("gif local table missing", &gif_with(8, 8, &frame, true));
    let frame = gif_frame(0, 0, 2, 2, 0, 2, &data);
    let bytes = gif_palette(8, 8, None, &frame, true);
    refused("gif no palette", &bytes, Format::Gif);
}

#[test]
fn a_gif_cut_or_with_blocks_that_lie_is_refused() {
    let good = gif_8x8();
    assert!(body("gif whole", &good).is_ok(), "the whole file decodes");
    // Every cut but the trailer's own: the frame is not all there.
    for cut in 0..good.len() - 1 {
        cut_refused(&format!("gif cut at {cut}"), &good[..cut], Format::Gif);
    }
    gif_settles("gif without its trailer", &good[..good.len() - 1]);
    // A sub-block longer than what is left, an extension with no end, and an
    // unknown block introducer.
    let long = gif_with(
        8,
        8,
        &[0x2C, 0, 0, 0, 0, 8, 0, 8, 0, 0, 2, 0xFF, 1, 2],
        false,
    );
    refused("gif sub-block runs off the end", &long, Format::Gif);
    let ext = gif_with(8, 8, &[0x21, 0xF9, 0xFF, 1, 2, 3], false);
    refused("gif extension without an end", &ext, Format::Gif);
    let unknown = gif_with(8, 8, &[0x99, 0x99, 0x99], false);
    refused("gif unknown block", &unknown, Format::Gif);
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

// --- All four ---------------------------------------------------------------

#[test]
fn bytes_that_pass_only_the_magic_are_refused() {
    let magics: [(&str, Format, Vec<u8>); 4] = [
        ("png", Format::Png, PNG_MAGIC.to_vec()),
        ("jpeg", Format::Jpeg, vec![0xFF, 0xD8, 0xFF]),
        ("gif", Format::Gif, b"GIF89a".to_vec()),
        ("webp", Format::Webp, riff(&[])),
    ];
    for (name, format, magic) in magics {
        for tail in [
            vec![],
            vec![0u8; 64],
            vec![0xFFu8; 64],
            (0..=255u8).collect(),
        ] {
            let mut bytes = magic.clone();
            bytes.extend_from_slice(&tail);
            refused(
                &format!("{name} magic and {} bytes", tail.len()),
                &bytes,
                format,
            );
        }
    }
}

// --- The ceilings, at the limit and just past it ---------------------------

/// A sample and what it decodes to.
struct Sample {
    name: &'static str,
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    frames: u32,
    /// The fewest bytes of ceiling it decodes in, as the budget counts them:
    /// the canvas for a PNG, the canvas and eight bytes a pixel of scratch for
    /// a JPEG, and so on. Pinned, so a ceiling off by one fails here.
    least: u64,
}

fn samples_all() -> Vec<Sample> {
    let (w, h) = (samples::W, samples::H);
    let small = samples::rgba(16, 12, true);
    vec![
        Sample {
            name: "png",
            bytes: samples::pngs().remove(0).1,
            width: w,
            height: h,
            frames: 1,
            least: 12288,
        },
        Sample {
            name: "jpeg",
            bytes: samples::jpeg(),
            width: w,
            height: h,
            frames: 1,
            least: 36864,
        },
        Sample {
            name: "gif",
            bytes: samples::gif_animated(),
            width: u32::from(samples::GIF_W),
            height: u32::from(samples::GIF_H),
            frames: 4,
            least: 21504,
        },
        Sample {
            name: "webp",
            bytes: samples::webp(&samples::rgba(w, h, true), w, h, true),
            width: w,
            height: h,
            frames: 1,
            least: 24576,
        },
        Sample {
            name: "webp-animated",
            bytes: samples::webp_animated(16, 12, &[(small.clone(), 30), (small, 70)]),
            width: 16,
            height: 12,
            frames: 2,
            least: 2304,
        },
    ]
}

/// Ceilings no sample reaches.
const ROOMY: Limits = Limits {
    max_input: 1 << 20,
    max_side: 4_096,
    max_pixels: 1 << 24,
    max_frames: 64,
    max_bytes: 1 << 28,
};

fn ok_under(s: &Sample, limits: &Limits, what: &str) {
    let name = format!("{} {what}", s.name);
    let (got, _) = run_under(&name, &s.bytes, limits);
    let image = got.unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert_eq!((image.width, image.height), (s.width, s.height), "{name}");
    assert_eq!(image.frames.len() as u32, s.frames, "{name}");
}

fn over(s: &Sample, limits: &Limits, ceiling: Ceiling, what: &str) {
    let name = format!("{} {what}", s.name);
    let (got, peak) = run_under(&name, &s.bytes, limits);
    assert_eq!(got.map(dims), Err(Error::TooBig(ceiling)), "{name}");
    // Refused, so what it made is under what the ceiling allowed, and the
    // sample's own size at that.
    assert!(
        peak <= limits.max_bytes as usize + SIZE_PEAK,
        "{name}: {peak} bytes"
    );
}

#[test]
fn each_ceiling_holds_at_its_limit_and_just_past_it() {
    for s in samples_all() {
        let (w, h) = (s.width, s.height);
        // The longest side.
        let at = Limits {
            max_side: w.max(h),
            ..ROOMY
        };
        ok_under(&s, &at, "at the side ceiling");
        over(
            &s,
            &Limits {
                max_side: w.max(h) - 1,
                ..ROOMY
            },
            Ceiling::Side,
            "past the side ceiling",
        );
        // The pixels.
        let at = Limits {
            max_pixels: u64::from(w) * u64::from(h),
            ..ROOMY
        };
        ok_under(&s, &at, "at the pixel ceiling");
        let past = Limits {
            max_pixels: u64::from(w) * u64::from(h) - 1,
            ..ROOMY
        };
        over(&s, &past, Ceiling::Pixels, "past the pixel ceiling");
        // The input's length.
        let at = Limits {
            max_input: s.bytes.len(),
            ..ROOMY
        };
        ok_under(&s, &at, "at the input ceiling");
        let past = Limits {
            max_input: s.bytes.len() - 1,
            ..ROOMY
        };
        over(&s, &past, Ceiling::Input, "past the input ceiling");
        // The frames.
        ok_under(
            &s,
            &Limits {
                max_frames: s.frames,
                ..ROOMY
            },
            "at the frame ceiling",
        );
        let past = Limits {
            max_frames: s.frames - 1,
            ..ROOMY
        };
        over(&s, &past, Ceiling::Frames, "past the frame ceiling");
    }
}

#[test]
fn the_byte_ceiling_holds_at_its_limit_and_just_past_it() {
    for s in samples_all() {
        let canvas = u64::from(s.width) * u64::from(s.height) * 4;
        let decodes = |max_bytes: u64| {
            let limits = Limits { max_bytes, ..ROOMY };
            run_under(s.name, &s.bytes, &limits).0
        };
        // The least the decode needs: found, then held to as the ceiling.
        let (mut low, mut high) = (0, 1u64 << 28);
        while low + 1 < high {
            let mid = (low + high) / 2;
            match decodes(mid) {
                Ok(_) => high = mid,
                Err(Error::TooBig(Ceiling::Bytes)) => low = mid,
                Err(other) => panic!("{} at {mid} bytes: {other:?}", s.name),
            }
        }
        let least = high;
        assert_eq!(least, s.least, "{}: the fewest bytes it decodes in", s.name);
        assert!(least >= canvas, "{}: the image itself is counted", s.name);
        let at = Limits {
            max_bytes: least,
            ..ROOMY
        };
        ok_under(&s, &at, "at the byte ceiling");
        let past = Limits {
            max_bytes: least - 1,
            ..ROOMY
        };
        over(&s, &past, Ceiling::Bytes, "past the byte ceiling");
        // At the ceiling the decode held no more than the ceiling and the input.
        let (_, peak) = run_under(s.name, &s.bytes, &at);
        assert!(
            peak as u64 <= least + s.bytes.len() as u64 + SIZE_PEAK as u64,
            "{}: peak {peak} at a ceiling of {least}",
            s.name
        );
    }
}

#[test]
fn a_file_refused_for_its_size_allocated_next_to_nothing() {
    for (name, bytes, ceiling) in [
        (
            "png",
            samples::png_claiming(100_000, 100_000),
            Ceiling::Side,
        ),
        ("png", samples::png_claiming(1_024, 1_025), Ceiling::Side),
        (
            "jpeg",
            samples::jpeg_claiming(65_000, 65_000),
            Ceiling::Side,
        ),
        ("jpeg", samples::jpeg_claiming(1_025, 1), Ceiling::Side),
        ("gif", samples::gif_claiming(65_535, 65_535), Ceiling::Side),
        ("gif", samples::gif_claiming(1_025, 1), Ceiling::Side),
        (
            "webp",
            samples::webp_claiming(20_000, 20_000),
            Ceiling::Side,
        ),
        ("webp", samples::webp_claiming(1_025, 1), Ceiling::Side),
    ] {
        refused_big(&format!("{name} {ceiling:?}"), &bytes, &LIMITS, ceiling);
    }
    // At the pixel ceiling under LIMITS: sides within it, the area past it.
    let tight = Limits {
        max_pixels: 100 * 100,
        ..LIMITS
    };
    for (name, bytes) in [
        ("png", samples::png_claiming(101, 100)),
        ("jpeg", samples::jpeg_claiming(101, 100)),
        ("gif", samples::gif_claiming(101, 100)),
        ("webp", samples::webp_claiming(101, 100)),
    ] {
        refused_big(&format!("{name} pixels"), &bytes, &tight, Ceiling::Pixels);
    }
}
