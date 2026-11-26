//! GIF: LZW streams, frames off the canvas, lying blocks and cuts (see `crafted`).

use super::crafted::*;
use kaimedia::{Ceiling, Format};

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
