//! PNG: hand-built chunks, headers, cuts and floods (see `crafted`).

use super::crafted::*;
use super::samples;
use kaimedia::{Ceiling, Error, Format, decode};

pub const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
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
