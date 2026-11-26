//! `kaimedia`: one decode API over PNG, JPEG, GIF and WebP (plan M-1's host
//! half, P6 groundwork G8). Every image comes out as RGBA8, one buffer per
//! frame, with each frame the whole canvas (an animation's frames are already
//! composed).
//!
//! The bytes are untrusted: a page, a mail or a download. So every decode
//! runs under [`Limits`], and each ceiling is checked against what the file's
//! header claims **before** the memory for it is asked for:
//! - the input's length;
//! - each side, and the pixel count of a canvas or frame;
//! - the frame count;
//! - the bytes decoded in all, output frames and scratch together.
//!
//! The decoders get the same byte ceiling for their own scratch, where they
//! take one. A file past a ceiling is refused with [`Error::TooBig`] naming it.
//! A file that is cut short, or that breaks its format, is [`Error::Bad`].
//! Nothing here panics on any input (`tests/hostile.rs`, and the fuzz target
//! `fuzz/fuzz_targets/media_decode.rs`).
//!
//! Not yet: colour profiles (ICC, gAMA) are ignored, so pixels are as stored.
//! An animated PNG gives its default image only. Nothing in the OS uses this
//! crate yet.

mod gif;
mod jpeg;
mod png;
mod webp;

use core::fmt;

/// A format [`sniff`] knows by its first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpeg",
            Format::Gif => "gif",
            Format::Webp => "webp",
        }
    }
}

/// The ceilings one decode runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes of file read.
    pub max_input: usize,
    /// The widest and tallest a canvas or frame may be, in pixels.
    pub max_side: u32,
    /// The most pixels in one canvas or frame.
    pub max_pixels: u64,
    /// The most frames an animation may have.
    pub max_frames: u32,
    /// The most bytes decoded in all: every RGBA frame returned, and the
    /// scratch a decode keeps beside them (a GIF's canvas, a frame before
    /// it is made RGBA).
    pub max_bytes: u64,
}

impl Limits {
    /// For an image shown on the desktop: 16,384 pixels a side, 32
    /// megapixels (128 MiB as RGBA), 1,000 frames, 256 MiB decoded in all,
    /// from at most 64 MiB of file.
    pub const DEFAULT: Limits = Limits {
        max_input: 64 << 20,
        max_side: 16_384,
        max_pixels: 32_000_000,
        max_frames: 1_000,
        max_bytes: 256 << 20,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Limits::DEFAULT
    }
}

/// Which ceiling a file went past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ceiling {
    Input,
    Side,
    Pixels,
    Frames,
    Bytes,
}

/// Why a decode was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The first bytes are none of the four formats.
    Unknown,
    /// The file claims more than a ceiling allows.
    TooBig(Ceiling),
    /// The file is cut short or breaks its format: the decoder's own words.
    Bad(Format, String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unknown => f.write_str("not a png, jpeg, gif or webp image"),
            Error::TooBig(c) => write!(f, "the image is too big ({c:?})"),
            Error::Bad(format, why) => write!(f, "a bad {} image: {why}", format.name()),
        }
    }
}

impl std::error::Error for Error {}

/// One frame: the whole canvas, RGBA8, row by row with no padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub rgba: Vec<u8>,
    /// How long it shows, in milliseconds; 0 for a still image.
    pub delay_ms: u32,
}

/// A decoded image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// At least one; more for an animation.
    pub frames: Vec<Frame>,
}

/// What a file's header says, without decoding its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// The frame count, when the header says it (GIF's does not).
    pub frames: Option<u32>,
}

/// The format `bytes` start like, if any.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::Webp)
    } else {
        None
    }
}

/// Decodes `bytes` under `limits`.
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, Error> {
    let format = start(bytes, limits)?;
    let mut budget = Budget::new(limits);
    match format {
        Format::Png => png::decode(bytes, &mut budget),
        Format::Jpeg => jpeg::decode(bytes, &mut budget),
        Format::Gif => gif::decode(bytes, &mut budget),
        Format::Webp => webp::decode(bytes, &mut budget),
    }
}

/// Reads `bytes`' header under `limits`, and checks it against them, without
/// decoding pixels.
pub fn probe(bytes: &[u8], limits: &Limits) -> Result<Info, Error> {
    let format = start(bytes, limits)?;
    let info = match format {
        Format::Png => png::probe(bytes, limits)?,
        Format::Jpeg => jpeg::probe(bytes, limits)?,
        Format::Gif => gif::probe(bytes, limits)?,
        Format::Webp => webp::probe(bytes, limits)?,
    };
    check_size(limits, info.width, info.height)?;
    if info.frames.is_some_and(|n| n > limits.max_frames) {
        return Err(Error::TooBig(Ceiling::Frames));
    }
    Ok(info)
}

fn start(bytes: &[u8], limits: &Limits) -> Result<Format, Error> {
    if bytes.len() > limits.max_input {
        return Err(Error::TooBig(Ceiling::Input));
    }
    sniff(bytes).ok_or(Error::Unknown)
}

/// Checks a canvas or frame of `width` x `height` against the side and pixel
/// ceilings.
pub(crate) fn check_size(limits: &Limits, width: u32, height: u32) -> Result<(), Error> {
    if width > limits.max_side || height > limits.max_side {
        return Err(Error::TooBig(Ceiling::Side));
    }
    if u64::from(width) * u64::from(height) > limits.max_pixels {
        return Err(Error::TooBig(Ceiling::Pixels));
    }
    Ok(())
}

/// RGBA8 bytes for `width` x `height`.
pub(crate) fn rgba_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 4
}

/// What one decode has asked for so far, held to [`Limits::max_bytes`] and
/// [`Limits::max_frames`]. Every buffer is taken from it before it is made.
pub(crate) struct Budget<'a> {
    pub(crate) limits: &'a Limits,
    used: u64,
    frames: u32,
}

impl<'a> Budget<'a> {
    fn new(limits: &'a Limits) -> Self {
        Budget {
            limits,
            used: 0,
            frames: 0,
        }
    }

    /// A zeroed buffer of `bytes`, once the budget has room for it.
    pub(crate) fn buffer(&mut self, bytes: u64) -> Result<Vec<u8>, Error> {
        let len = usize::try_from(bytes).map_err(|_| Error::TooBig(Ceiling::Bytes))?;
        self.reserve(bytes)?;
        Ok(vec![0; len])
    }

    /// Counts `bytes` a decoder will make out of sight (its planes and
    /// coefficients) against the budget, before it makes them.
    pub(crate) fn reserve(&mut self, bytes: u64) -> Result<(), Error> {
        self.used = self
            .used
            .checked_add(bytes)
            .filter(|&used| used <= self.limits.max_bytes)
            .ok_or(Error::TooBig(Ceiling::Bytes))?;
        Ok(())
    }

    /// Counts one more frame, refused past [`Limits::max_frames`].
    pub(crate) fn frame(&mut self) -> Result<(), Error> {
        if self.frames >= self.limits.max_frames {
            return Err(Error::TooBig(Ceiling::Frames));
        }
        self.frames += 1;
        Ok(())
    }

    /// What is left for a decoder's own scratch, for those that take a cap.
    pub(crate) fn left(&self) -> u64 {
        self.limits.max_bytes - self.used
    }
}

/// Widens `pixels` of `channels` 8-bit samples (1 gray, 2 gray + alpha, 3
/// RGB, 4 RGBA) to RGBA8 in `out`, which holds exactly that many pixels.
pub(crate) fn to_rgba(pixels: &[u8], channels: usize, out: &mut [u8]) {
    for (src, dst) in pixels
        .chunks_exact(channels)
        .zip(out.as_chunks_mut::<4>().0)
    {
        *dst = match *src {
            [g] => [g, g, g, 255],
            [g, a] => [g, g, g, a],
            [r, g, b] => [r, g, b, 255],
            [r, g, b, a] => [r, g, b, a],
            _ => [0, 0, 0, 0],
        };
    }
}

#[cfg(test)]
mod tests;
