//! The test images, made here from [`pattern`] with each format's own encoder
//! (`png`, `gif` and `image-webp` encode; nothing binary is committed but one
//! JPEG, `data/gradient.jpg`, made from the same pattern, since no encoder
//! with an allowed licence is in the tree). `goldens.rs` checks each decodes to
//! the pixels it was made from; `hostile.rs` mutates and cuts every one.

#![allow(dead_code)]

/// The goldens' size: small, but not a multiple of 8 one way.
pub const W: u32 = 64;
pub const H: u32 = 48;

/// The pixel at (x, y) of a `w` x `h` sample: gradients in red, green and
/// blue, and with `alpha` an alpha falling left to right.
pub fn pattern(x: u32, y: u32, w: u32, h: u32, alpha: bool) -> [u8; 4] {
    let r = (x * 255 / (w - 1)) as u8;
    let g = (y * 255 / (h - 1)) as u8;
    let b = ((x + y) * 255 / (w + h - 2)) as u8;
    let a = if alpha {
        255 - (x * 200 / (w - 1)) as u8
    } else {
        255
    };
    [r, g, b, a]
}

/// [`pattern`] over the whole sample, as RGBA8.
pub fn rgba(w: u32, h: u32, alpha: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            out.extend_from_slice(&pattern(x, y, w, h, alpha));
        }
    }
    out
}

fn png_with(
    w: u32,
    h: u32,
    color: png::ColorType,
    depth: png::BitDepth,
    data: &[u8],
    palette: Option<(&[u8], &[u8])>,
) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(color);
        encoder.set_depth(depth);
        if let Some((plte, trns)) = palette {
            encoder.set_palette(plte.to_vec());
            encoder.set_trns(trns.to_vec());
        }
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(data).unwrap();
    }
    out
}

/// The PNG samples, each with the RGBA it must decode to.
pub fn pngs() -> Vec<(&'static str, Vec<u8>, Vec<u8>)> {
    use png::{BitDepth, ColorType};
    let rgba8 = rgba(W, H, true);
    let opaque = rgba(W, H, false);
    let rgb8: Vec<u8> = opaque.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let gray: Vec<u8> = opaque.chunks(4).map(|p| p[0]).collect();
    let gray_rgba: Vec<u8> = gray.iter().flat_map(|&g| [g, g, g, 255]).collect();
    let gray_alpha: Vec<u8> = rgba8.chunks(4).flat_map(|p| [p[0], p[3]]).collect();
    let gray_alpha_rgba: Vec<u8> = rgba8
        .chunks(4)
        .flat_map(|p| [p[0], p[0], p[0], p[3]])
        .collect();
    // 16-bit RGB whose high bytes are the pattern: cut to 8 it is the pattern.
    let rgb16: Vec<u8> = rgb8.iter().flat_map(|&v| [v, 0x5A]).collect();
    let plte = [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255];
    let trns = [255, 128, 0, 255];
    let indexed: Vec<u8> = (0..H)
        .flat_map(|y| (0..W).map(move |x| ((x / 16 + y / 12) % 4) as u8))
        .collect();
    let indexed_rgba: Vec<u8> = indexed
        .iter()
        .flat_map(|&i| {
            let i = usize::from(i);
            [plte[i * 3], plte[i * 3 + 1], plte[i * 3 + 2], trns[i]]
        })
        .collect();
    vec![
        (
            "rgba8",
            png_with(W, H, ColorType::Rgba, BitDepth::Eight, &rgba8, None),
            rgba8.clone(),
        ),
        (
            "rgb8",
            png_with(W, H, ColorType::Rgb, BitDepth::Eight, &rgb8, None),
            opaque.clone(),
        ),
        (
            "gray8",
            png_with(W, H, ColorType::Grayscale, BitDepth::Eight, &gray, None),
            gray_rgba,
        ),
        (
            "gray-alpha",
            png_with(
                W,
                H,
                ColorType::GrayscaleAlpha,
                BitDepth::Eight,
                &gray_alpha,
                None,
            ),
            gray_alpha_rgba,
        ),
        (
            "rgb16",
            png_with(W, H, ColorType::Rgb, BitDepth::Sixteen, &rgb16, None),
            opaque,
        ),
        (
            "indexed-trns",
            png_with(
                W,
                H,
                ColorType::Indexed,
                BitDepth::Eight,
                &indexed,
                Some((&plte, &trns)),
            ),
            indexed_rgba,
        ),
    ]
}

/// The animated GIF's palette: transparent black, red, green, blue.
pub const GIF_COLOURS: [[u8; 4]; 4] = [
    [0, 0, 0, 0],
    [255, 0, 0, 255],
    [0, 255, 0, 255],
    [0, 0, 255, 255],
];
pub const GIF_W: u16 = 32;
pub const GIF_H: u16 = 24;

/// Four frames on a 32 x 24 canvas, one of each disposal, one reaching past
/// the canvas and one with transparent pixels (see goldens.rs for what each
/// frame must show).
pub fn gif_animated() -> Vec<u8> {
    use gif::{DisposalMethod, Encoder, Frame, Repeat};
    let palette: Vec<u8> = GIF_COLOURS
        .iter()
        .flat_map(|c| [c[0], c[1], c[2]])
        .collect();
    let mut out = Vec::new();
    {
        let mut encoder = Encoder::new(&mut out, GIF_W, GIF_H, &palette).unwrap();
        encoder.set_repeat(Repeat::Infinite).unwrap();
        let full: Vec<u8> = (0..GIF_H)
            .flat_map(|y| (0..GIF_W).map(move |x| ((x + y) % 3 + 1) as u8))
            .collect();
        let mut f = Frame::from_indexed_pixels(GIF_W, GIF_H, full, None);
        f.delay = 5;
        f.dispose = DisposalMethod::Keep;
        encoder.write_frame(&f).unwrap();
        let checker: Vec<u8> = (0..8u16)
            .flat_map(|y| (0..8u16).map(move |x| if (x + y) % 2 == 0 { 1 } else { 0 }))
            .collect();
        let mut f = Frame::from_indexed_pixels(8, 8, checker, Some(0));
        (f.left, f.top, f.delay, f.dispose) = (8, 4, 10, DisposalMethod::Background);
        encoder.write_frame(&f).unwrap();
        let mut f = Frame::from_indexed_pixels(20, 20, vec![3u8; 400], None);
        (f.left, f.top, f.delay, f.dispose) = (20, 10, 20, DisposalMethod::Previous);
        encoder.write_frame(&f).unwrap();
        let mut f = Frame::from_indexed_pixels(1, 1, vec![2u8], None);
        (f.delay, f.dispose) = (0, DisposalMethod::Keep);
        encoder.write_frame(&f).unwrap();
    }
    out
}

/// A GIF of `frames` 1 x 1 frames, for the frame ceiling.
pub fn gif_frames(frames: usize) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(&mut out, 1, 1, &[0, 0, 0, 255, 255, 255]).unwrap();
        for n in 0..frames {
            encoder
                .write_frame(&gif::Frame::from_indexed_pixels(
                    1,
                    1,
                    vec![(n % 2) as u8],
                    None,
                ))
                .unwrap();
        }
    }
    out
}

/// A still lossless WebP of `data` (RGBA8 or RGB8).
pub fn webp(data: &[u8], w: u32, h: u32, alpha: bool) -> Vec<u8> {
    use image_webp::{ColorType, WebPEncoder};
    let mut out = Vec::new();
    let color = if alpha {
        ColorType::Rgba8
    } else {
        ColorType::Rgb8
    };
    WebPEncoder::new(&mut out)
        .encode(data, w, h, color)
        .unwrap();
    out
}

fn chunk(out: &mut Vec<u8>, name: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(name);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

fn u24(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes()[..3]);
}

/// An animated WebP on a `w` x `h` canvas: each of `frames` (RGBA8, the whole
/// canvas, shown for its milliseconds) encoded alone, its `VP8L` chunk put in
/// an `ANMF` frame with no blending and no disposal. image-webp encodes stills
/// only, so the container is made here (WebP container spec, "Animation").
pub fn webp_animated(w: u32, h: u32, frames: &[(Vec<u8>, u32)]) -> Vec<u8> {
    let mut body = b"WEBP".to_vec();
    let mut vp8x = vec![0x02 | 0x10, 0, 0, 0];
    u24(&mut vp8x, w - 1);
    u24(&mut vp8x, h - 1);
    chunk(&mut body, b"VP8X", &vp8x);
    chunk(&mut body, b"ANIM", &[0, 0, 0, 0, 0, 0]);
    for (rgba, ms) in frames {
        let still = webp(rgba, w, h, true);
        // The still is RIFF, size, WEBP, then its VP8L chunk.
        let vp8l = &still[12..];
        let mut anmf = Vec::new();
        u24(&mut anmf, 0);
        u24(&mut anmf, 0);
        u24(&mut anmf, w - 1);
        u24(&mut anmf, h - 1);
        u24(&mut anmf, *ms);
        // Blending off (bit 1), disposal none (bit 0 clear).
        anmf.push(0x02);
        anmf.extend_from_slice(vp8l);
        chunk(&mut body, b"ANMF", &anmf);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// The committed JPEG: [`pattern`] at `W` x `H`, opaque, baseline 4:2:0,
/// made with macOS `sips` from a PNG of it.
pub fn jpeg() -> Vec<u8> {
    include_bytes!("../data/gradient.jpg").to_vec()
}

/// A PNG chunk with its CRC (for headers made by hand).
pub fn png_chunk(out: &mut Vec<u8>, name: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(name);
    out.extend_from_slice(body);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A PNG whose header claims `w` x `h` RGBA8, with a few bytes of deflate
/// after it: a decompression bomb's header.
pub fn png_claiming(w: u32, h: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    // zlib: a stored-free stream of zeros would still be short; any bytes do.
    png_chunk(&mut out, b"IDAT", &[0x78, 0x9C, 0x63, 0, 0, 0, 1, 0, 1]);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

/// [`jpeg`] with its frame header saying `w` x `h`.
pub fn jpeg_claiming(w: u16, h: u16) -> Vec<u8> {
    let mut bytes = jpeg();
    let sof = bytes
        .windows(2)
        .position(|m| m[0] == 0xFF && matches!(m[1], 0xC0..=0xC2))
        .expect("a frame header");
    // FF Cn, length (2), precision (1), height (2), width (2).
    bytes[sof + 5..sof + 7].copy_from_slice(&h.to_be_bytes());
    bytes[sof + 7..sof + 9].copy_from_slice(&w.to_be_bytes());
    bytes
}

/// A GIF whose logical screen is `w` x `h`, with one 1 x 1 frame.
pub fn gif_claiming(w: u16, h: u16) -> Vec<u8> {
    let mut bytes = gif_frames(1);
    bytes[6..8].copy_from_slice(&w.to_le_bytes());
    bytes[8..10].copy_from_slice(&h.to_le_bytes());
    bytes
}

/// A WebP whose extended header claims a `w` x `h` canvas, animated, with one
/// 1 x 1 frame.
pub fn webp_claiming(w: u32, h: u32) -> Vec<u8> {
    let mut bytes = webp_animated(1, 1, &[(vec![1, 2, 3, 255], 10)]);
    // RIFF, size, WEBP, then VP8X: name, size, flags (4), width - 1, height - 1.
    bytes[24..27].copy_from_slice(&(w - 1).to_le_bytes()[..3]);
    bytes[27..30].copy_from_slice(&(h - 1).to_le_bytes()[..3]);
    bytes
}
