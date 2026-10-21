//! Where text goes on the framebuffer: the font size, the margins, the status bar
//! at the top and the spacing between rows.
//!
//! The font is Noto Sans Mono, pre-rasterised with 8-bit coverage (anti-aliased)
//! by the noto-sans-mono-bitmap crate, in 16, 20, 24 and 32 pixel rows. The console
//! takes the largest size that still gives `MIN_COLS` columns, so the 160-character
//! pieces the terminal prints (`terminal::PIECE_CHARS`) keep fitting on one row and
//! nothing printed, on screen or on serial, wraps differently from one screen to
//! the next: 16 px at 1280x800, 20 at 1600x900, 24 at 1920x1080, 32 at 2560x1440.
//! Narrower screens get 16 px with slimmer margins.

use noto_sans_mono_bitmap::{
    FontWeight, RasterHeight, RasterizedChar, get_raster, get_raster_width,
};

/// Columns the chosen font must leave: a piece and its two-character lead.
const MIN_COLS: usize = crate::terminal::PIECE_CHARS + 2;
/// Sizes to try, largest first; the last is the fallback.
const SIZES: [RasterHeight; 4] = [
    RasterHeight::Size32,
    RasterHeight::Size24,
    RasterHeight::Size20,
    RasterHeight::Size16,
];
/// Margin on each side, in cells, on screens wide enough for it.
const MARGIN_CELLS: usize = 3;
/// Below this width the margin shrinks to one cell.
const NARROW: usize = 1024;
const REPLACEMENT: char = '\u{FFFD}';

/// One size of the font.
#[derive(Clone, Copy)]
pub struct Font {
    size: RasterHeight,
    /// A cell in pixels.
    pub width: usize,
    pub height: usize,
}

impl Font {
    const fn new(size: RasterHeight) -> Self {
        Self {
            size,
            width: get_raster_width(FontWeight::Regular, size),
            height: size as usize,
        }
    }

    /// The glyph for `ch`, or the replacement character's; None for a blank.
    pub fn glyph(self, ch: char, bold: bool) -> Option<RasterizedChar> {
        let weight = if bold {
            FontWeight::Bold
        } else {
            FontWeight::Regular
        };
        match ch {
            ' ' | '\0' => None,
            ch => get_raster(ch, weight, self.size)
                .or_else(|| get_raster(REPLACEMENT, weight, self.size)),
        }
    }
}

/// The smallest font, which the pointer's cells and a console-less boot assume.
pub const SMALL: Font = Font::new(RasterHeight::Size16);

/// The console's geometry, in pixels.
#[derive(Clone, Copy)]
pub struct Layout {
    pub font: Font,
    /// From the top of one text row to the top of the next: the font's height and
    /// an eighth more, so lines breathe.
    pub pitch: usize,
    /// The top left of the text area.
    pub left: usize,
    pub top: usize,
    pub cols: usize,
    pub rows: usize,
    /// Height of the status bar across the top.
    pub bar: usize,
    pub width: usize,
}

impl Layout {
    /// The layout for a screen of `width` by `height` pixels.
    pub fn new(width: usize, height: usize) -> Self {
        SIZES
            .iter()
            .map(|&size| Self::with_font(Font::new(size), width, height))
            .find(|layout| layout.cols >= MIN_COLS)
            .unwrap_or_else(|| Self::with_font(SMALL, width, height))
    }

    fn with_font(font: Font, width: usize, height: usize) -> Self {
        let margin = match width >= NARROW {
            true => MARGIN_CELLS * font.width,
            false => font.width,
        };
        let cols = width.saturating_sub(2 * margin) / font.width;
        let pitch = font.height + font.height / 8;
        let bar = font.height + 2 * (font.height / 3);
        let gap = font.height / 2;
        let top = bar + gap;
        let rows = height.saturating_sub(top + gap) / pitch;
        Self {
            font,
            pitch,
            left: (width - cols * font.width) / 2,
            top,
            cols,
            rows,
            bar,
            width,
        }
    }

    /// The top left pixel of a cell.
    pub fn cell(&self, row: usize, col: usize) -> (usize, usize) {
        (
            self.left + col * self.font.width,
            self.top + row * self.pitch,
        )
    }

    /// The margin bar of a row: its left edge and width.
    pub fn bar_x(&self) -> (usize, usize) {
        let width = (self.font.width / 3).max(2);
        (self.left.saturating_sub(self.font.width), width)
    }
}
