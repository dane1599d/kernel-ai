//! PNG through `png` 0.18: the header is read, and checked against the
//! ceilings, before the image's buffer is made; `png`'s own scratch (the
//! chunks it keeps, its inflater) is held to what the budget has left. 16-bit
//! samples are cut to 8, palettes and low bit depths widened, and every colour
//! type made RGBA. An animated PNG gives its default image.

use crate::{Budget, Error, Format, Frame, Image, Info, Limits, check_size, rgba_bytes, to_rgba};
use png::{ColorType, Decoder, Transformations};
use std::io::Cursor;

fn bad(e: png::DecodingError) -> Error {
    match e {
        png::DecodingError::LimitsExceeded => Error::TooBig(crate::Ceiling::Bytes),
        e => Error::Bad(Format::Png, e.to_string()),
    }
}

fn reader(bytes: &[u8], scratch: u64) -> Result<png::Reader<Cursor<&[u8]>>, Error> {
    let limits = png::Limits {
        bytes: usize::try_from(scratch).unwrap_or(usize::MAX),
    };
    let mut decoder = Decoder::new_with_limits(Cursor::new(bytes), limits);
    decoder.set_transformations(Transformations::normalize_to_color8());
    decoder.read_info().map_err(bad)
}

pub(crate) fn probe(bytes: &[u8], limits: &Limits) -> Result<Info, Error> {
    let reader = reader(bytes, limits.max_bytes)?;
    let info = reader.info();
    Ok(Info {
        format: Format::Png,
        width: info.width,
        height: info.height,
        frames: Some(1),
    })
}

pub(crate) fn decode(bytes: &[u8], budget: &mut Budget) -> Result<Image, Error> {
    let mut reader = reader(bytes, budget.left())?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    check_size(budget.limits, width, height)?;
    let (color, _) = reader.output_color_type();
    let channels = match color {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        // `normalize_to_color8` expands palettes.
        ColorType::Indexed => {
            return Err(Error::Bad(
                Format::Png,
                String::from("a palette left unexpanded"),
            ));
        }
    };
    let size = reader
        .output_buffer_size()
        .ok_or(Error::TooBig(crate::Ceiling::Bytes))?;
    budget.frame()?;
    let mut raw = budget.buffer(size as u64)?;
    let out = reader.next_frame(&mut raw).map_err(bad)?;
    if (out.width, out.height) != (width, height) {
        return Err(Error::Bad(
            Format::Png,
            String::from("its first frame is not the whole image"),
        ));
    }
    let rgba = if channels == 4 && out.line_size == width as usize * 4 {
        raw.truncate(rgba_bytes(width, height) as usize);
        raw
    } else {
        let mut rgba = budget.buffer(rgba_bytes(width, height))?;
        for (row, dst) in raw
            .chunks(out.line_size)
            .take(height as usize)
            .zip(rgba.chunks_exact_mut(width as usize * 4))
        {
            to_rgba(&row[..width as usize * channels], channels, dst);
        }
        rgba
    };
    Ok(Image {
        format: Format::Png,
        width,
        height,
        frames: vec![Frame { rgba, delay_ms: 0 }],
    })
}
