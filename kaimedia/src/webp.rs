//! WebP through `image-webp` 0.2: the header gives the canvas and, for an
//! animation, the frame count, so the whole decode (every frame, as RGBA) is
//! checked against the ceilings before any of it is made. `image-webp`
//! composes an animation's frames itself, and its own scratch is held to what
//! the budget has left.

use crate::{
    Budget, Ceiling, Error, Format, Frame, Image, Info, Limits, check_size, rgba_bytes, to_rgba,
};
use image_webp::{DecodingError, WebPDecoder};
use std::io::Cursor;

fn bad(e: DecodingError) -> Error {
    match e {
        DecodingError::MemoryLimitExceeded => Error::TooBig(Ceiling::Bytes),
        e => Error::Bad(Format::Webp, e.to_string()),
    }
}

fn open(bytes: &[u8]) -> Result<WebPDecoder<Cursor<&[u8]>>, Error> {
    WebPDecoder::new(Cursor::new(bytes)).map_err(bad)
}

fn frames_of(decoder: &WebPDecoder<Cursor<&[u8]>>) -> u32 {
    if decoder.is_animated() {
        decoder.num_frames()
    } else {
        1
    }
}

pub(crate) fn probe(bytes: &[u8], _: &Limits) -> Result<Info, Error> {
    let decoder = open(bytes)?;
    let (width, height) = decoder.dimensions();
    Ok(Info {
        format: Format::Webp,
        width,
        height,
        frames: Some(frames_of(&decoder)),
    })
}

pub(crate) fn decode(bytes: &[u8], budget: &mut Budget) -> Result<Image, Error> {
    let mut decoder = open(bytes)?;
    let (width, height) = decoder.dimensions();
    check_size(budget.limits, width, height)?;
    let count = frames_of(&decoder);
    if count == 0 {
        return Err(Error::Bad(
            Format::Webp,
            String::from("an animation with no frames"),
        ));
    }
    if count > budget.limits.max_frames {
        return Err(Error::TooBig(Ceiling::Frames));
    }
    let size = rgba_bytes(width, height);
    // Every frame, before the first is made.
    let all = size
        .checked_mul(u64::from(count))
        .ok_or(Error::TooBig(Ceiling::Bytes))?;
    if all > budget.left() {
        return Err(Error::TooBig(Ceiling::Bytes));
    }
    let raw_len = decoder
        .output_buffer_size()
        .ok_or(Error::TooBig(Ceiling::Bytes))?;
    let channels = if decoder.has_alpha() { 4 } else { 3 };
    let mut raw = budget.buffer(raw_len as u64)?;
    decoder.set_memory_limit(usize::try_from(budget.left()).unwrap_or(usize::MAX));
    let mut frames = Vec::with_capacity(count as usize);
    for _ in 0..count {
        budget.frame()?;
        let delay_ms = if decoder.is_animated() {
            decoder.read_frame(&mut raw).map_err(bad)?
        } else {
            decoder.read_image(&mut raw).map_err(bad)?;
            0
        };
        let mut rgba = budget.buffer(size)?;
        to_rgba(&raw, channels, &mut rgba);
        frames.push(Frame { rgba, delay_ms });
    }
    if frames.len() == 1 {
        frames[0].delay_ms = 0;
    }
    Ok(Image {
        format: Format::Webp,
        width,
        height,
        frames,
    })
}
