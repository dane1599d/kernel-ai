//! GIF through `gif` 0.14, composed here: each frame is drawn onto the
//! canvas at its place, its transparent pixels leaving what is under them,
//! and the canvas is copied out as that frame. Then the frame's disposal is
//! applied: keep it, clear its rectangle, or put back the canvas from before
//! it.
//!
//! The canvas (the logical screen) is checked against the ceilings before it
//! is made. Each frame's rectangle is checked before its pixels are read, and
//! each frame is counted against the frame ceiling and the byte budget before
//! its copy is made. A frame's rectangle may reach past the canvas; only the
//! part on it is drawn.

use crate::{Budget, Error, Format, Frame, Image, Info, Limits, check_size, rgba_bytes};
use gif::{ColorOutput, DecodeOptions, DisposalMethod, MemoryLimit};
use std::io::Cursor;
use std::num::NonZeroU64;

fn bad(e: gif::DecodingError) -> Error {
    Error::Bad(Format::Gif, e.to_string())
}

fn open(bytes: &[u8], scratch: u64) -> Result<gif::Decoder<Cursor<&[u8]>>, Error> {
    let mut options = DecodeOptions::new();
    options.set_color_output(ColorOutput::RGBA);
    // One frame's RGBA rectangle at most; 0 would mean no limit.
    options.set_memory_limit(MemoryLimit::Bytes(
        NonZeroU64::new(scratch.max(1)).unwrap_or(NonZeroU64::MIN),
    ));
    options.read_info(Cursor::new(bytes)).map_err(bad)
}

pub(crate) fn probe(bytes: &[u8], limits: &Limits) -> Result<Info, Error> {
    let decoder = open(bytes, limits.max_bytes)?;
    Ok(Info {
        format: Format::Gif,
        width: u32::from(decoder.width()),
        height: u32::from(decoder.height()),
        frames: None,
    })
}

/// A frame's place, read before its pixels.
struct Place {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    dispose: DisposalMethod,
    delay_ms: u32,
}

pub(crate) fn decode(bytes: &[u8], budget: &mut Budget) -> Result<Image, Error> {
    let mut decoder = open(bytes, budget.left())?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    check_size(budget.limits, width, height)?;
    if width == 0 || height == 0 {
        return Err(Error::Bad(
            Format::Gif,
            String::from("a canvas with a side of 0"),
        ));
    }
    let size = rgba_bytes(width, height);
    let mut canvas = budget.buffer(size)?;
    let mut before: Option<Vec<u8>> = None;
    let mut pixels: Vec<u8> = Vec::new();
    let mut frames = Vec::new();
    loop {
        let place = match decoder.next_frame_info().map_err(bad)? {
            None => break,
            Some(f) => Place {
                left: u32::from(f.left),
                top: u32::from(f.top),
                width: u32::from(f.width),
                height: u32::from(f.height),
                dispose: f.dispose,
                delay_ms: u32::from(f.delay) * 10,
            },
        };
        check_size(budget.limits, place.width, place.height)?;
        budget.frame()?;
        let rect = rgba_bytes(place.width, place.height);
        if rect as usize != decoder.buffer_size() {
            return Err(Error::Bad(
                Format::Gif,
                String::from("a frame's size and its pixels disagree"),
            ));
        }
        // The scratch for a frame's pixels grows to the largest frame, and
        // only its growth is counted.
        if rect as usize > pixels.len() {
            let grow = rect - pixels.len() as u64;
            budget.reserve(grow)?;
            pixels.resize(rect as usize, 0);
        }
        decoder
            .read_into_buffer(&mut pixels[..rect as usize])
            .map_err(bad)?;
        if place.dispose == DisposalMethod::Previous {
            match before.as_mut() {
                Some(saved) => saved.copy_from_slice(&canvas),
                None => {
                    let mut saved = budget.buffer(size)?;
                    saved.copy_from_slice(&canvas);
                    before = Some(saved);
                }
            }
        }
        draw(&mut canvas, width, height, &place, &pixels[..rect as usize]);
        let mut rgba = budget.buffer(size)?;
        rgba.copy_from_slice(&canvas);
        frames.push(Frame {
            rgba,
            delay_ms: place.delay_ms,
        });
        match place.dispose {
            DisposalMethod::Background => clear(&mut canvas, width, height, &place),
            DisposalMethod::Previous => {
                if let Some(saved) = &before {
                    canvas.copy_from_slice(saved);
                }
            }
            DisposalMethod::Any | DisposalMethod::Keep => {}
        }
    }
    if frames.is_empty() {
        return Err(Error::Bad(Format::Gif, String::from("no frames")));
    }
    if frames.len() == 1 {
        frames[0].delay_ms = 0;
    }
    Ok(Image {
        format: Format::Gif,
        width,
        height,
        frames,
    })
}

/// The rows and columns of `place` that lie on a `width` x `height` canvas:
/// (first column, columns, first row, rows).
fn on_canvas(width: u32, height: u32, place: &Place) -> (usize, usize, usize, usize) {
    let x0 = place.left.min(width);
    let y0 = place.top.min(height);
    let x1 = place.left.saturating_add(place.width).min(width);
    let y1 = place.top.saturating_add(place.height).min(height);
    (
        x0 as usize,
        (x1 - x0) as usize,
        y0 as usize,
        (y1 - y0) as usize,
    )
}

fn draw(canvas: &mut [u8], width: u32, height: u32, place: &Place, pixels: &[u8]) {
    let (x0, cols, y0, rows) = on_canvas(width, height, place);
    let stride = width as usize * 4;
    let src_stride = place.width as usize * 4;
    for row in 0..rows {
        let src = &pixels[row * src_stride..][..cols * 4];
        let dst = &mut canvas[(y0 + row) * stride + x0 * 4..][..cols * 4];
        for (s, d) in src
            .as_chunks::<4>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<4>().0)
        {
            // GIF's alpha is 0 or 255: transparent leaves what is under it.
            if s[3] != 0 {
                *d = *s;
            }
        }
    }
}

fn clear(canvas: &mut [u8], width: u32, height: u32, place: &Place) {
    let (x0, cols, y0, rows) = on_canvas(width, height, place);
    let stride = width as usize * 4;
    for row in 0..rows {
        canvas[(y0 + row) * stride + x0 * 4..][..cols * 4].fill(0);
    }
}
