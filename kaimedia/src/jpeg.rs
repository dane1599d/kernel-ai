//! JPEG through `zune-jpeg` 0.5: the headers are read first, and the frame's
//! size checked against the ceilings, before anything the size of the image is
//! made. zune's own limits are set to the format's widest (65,535), so the
//! refusal is ours and names its ceiling. Its planes and coefficients are
//! out of sight, so the budget counts an estimate of them
//! ([`SCRATCH_PER_PIXEL`]) before the decode.

use crate::{Budget, Ceiling, Error, Format, Frame, Image, Info, Limits, check_size, rgba_bytes};
use std::io::Cursor;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Bytes a pixel of zune's scratch is counted as: up to four components'
/// 16-bit coefficients for a progressive file, past what a baseline one keeps.
pub(crate) const SCRATCH_PER_PIXEL: u64 = 8;

fn bad(e: impl core::fmt::Debug) -> Error {
    Error::Bad(Format::Jpeg, format!("{e:?}"))
}

/// A decoder past the headers, with the frame's width and height.
type Headed<'a> = (JpegDecoder<Cursor<&'a [u8]>>, u32, u32);

fn headers(bytes: &[u8]) -> Result<Headed<'_>, Error> {
    let options = DecoderOptions::default()
        .set_max_width(usize::from(u16::MAX))
        .set_max_height(usize::from(u16::MAX))
        .jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(Cursor::new(bytes), options);
    decoder.decode_headers().map_err(bad)?;
    let info = decoder.info().ok_or_else(|| bad("no frame header"))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    if width == 0 || height == 0 {
        return Err(bad("a side of 0"));
    }
    Ok((decoder, width, height))
}

pub(crate) fn probe(bytes: &[u8], _: &Limits) -> Result<Info, Error> {
    let (_, width, height) = headers(bytes)?;
    Ok(Info {
        format: Format::Jpeg,
        width,
        height,
        frames: Some(1),
    })
}

pub(crate) fn decode(bytes: &[u8], budget: &mut Budget) -> Result<Image, Error> {
    let (mut decoder, width, height) = headers(bytes)?;
    check_size(budget.limits, width, height)?;
    let size = decoder
        .output_buffer_size()
        .ok_or(Error::TooBig(Ceiling::Bytes))?;
    if size as u64 != rgba_bytes(width, height) {
        return Err(bad("not decoded as RGBA"));
    }
    budget.frame()?;
    budget.reserve(u64::from(width) * u64::from(height) * SCRATCH_PER_PIXEL)?;
    let mut rgba = budget.buffer(size as u64)?;
    decoder.decode_into(&mut rgba).map_err(bad)?;
    Ok(Image {
        format: Format::Jpeg,
        width,
        height,
        frames: vec![Frame { rgba, delay_ms: 0 }],
    })
}
