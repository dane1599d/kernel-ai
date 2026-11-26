//! What the hand-built inputs share (G27's kaimedia half; `png`, `jpeg`, `gif`,
//! `webp` and `ceilings` hold them): inputs built to pass each format's magic
//! and reach the deep
//! paths: zero and enormous sizes, chunks and segments
//! cut or lying about their length, JPEG Huffman and quantisation tables that
//! cannot be right, GIF LZW codes past the table, WebP frames off the canvas.
//! Each goes through `body`, which a fuzz target can call: it decodes under
//! small ceilings and checks that what decodes keeps them and that nothing
//! was allocated far past them. A refusal is fine; a panic, a hang or an
//! image past a ceiling is the bug.

use super::{harness, hostile};
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
pub const PEAK: usize = LIMITS.max_bytes as usize + (1 << 20);

/// The most a file refused for its claimed size may have allocated: a few
/// buffers and the decoders' fixed state (a PNG's inflate window), never the
/// claim, which is up to 4 GiB or more.
pub const SIZE_PEAK: usize = 256 << 10;

/// `bytes` under `limits`: the result and the peak allocation, with every
/// ceiling checked on what decoded.
pub fn run_under(name: &str, bytes: &[u8], limits: &Limits) -> (Result<Image, Error>, usize) {
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
pub fn refused(name: &str, bytes: &[u8], format: Format) {
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
pub fn refused_big(name: &str, bytes: &[u8], limits: &Limits, ceiling: Ceiling) {
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
/// never a ceiling. Whether it decoded: a case that must pin which is the
/// refusal collects these and compares them to the list it expects.
pub fn settles(name: &str, bytes: &[u8], format: Format, w: u32, h: u32) -> bool {
    match body(name, bytes) {
        Ok(image) => {
            assert_eq!((image.width, image.height), (w, h), "{name}");
            true
        }
        Err(Error::Bad(got, _)) => {
            assert_eq!(got, format, "{name}");
            false
        }
        Err(other) => panic!("{name}: {other:?}"),
    }
}

/// A file cut short: refused as a bad `format` once enough is left to name it,
/// and as of no format before that.
pub fn cut_refused(name: &str, bytes: &[u8], format: Format) {
    match kaimedia::sniff(bytes) {
        Some(_) => refused(name, bytes, format),
        None => assert_eq!(body(name, bytes).map(dims), Err(Error::Unknown), "{name}"),
    }
}

pub fn dims(image: Image) -> (u32, u32, usize) {
    (image.width, image.height, image.frames.len())
}
