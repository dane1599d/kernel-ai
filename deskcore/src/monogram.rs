//! A window's monogram (ADR 0005 §7): for an app with no glyph of ours, the
//! first letter of its kernel label in Label, `muted` on a 16 px `hover`
//! tile. The letter is centred on its own ink, not its advance, and drawn
//! smaller when the theme's text would not fit the tile (150-200% text).

use crate::geom::Rect;
use crate::paint::{Painter, fill, g, monogram_ink};
use alloc::vec::Vec;
use gfx2d::{Canvas, Image};
use look::TypeStyle;

/// The tile's side (`look::metric::ICON_TITLE`).
pub(crate) const TILE: i32 = look::metric::ICON_TITLE as i32;
/// The least margin between the letter's ink and the tile's edge.
const INSET: i32 = 3;
/// The scratch the ink is measured on: room for a 200% letter either way.
const SCRATCH: i32 = 96;
/// Where the measured letter's pen starts on the scratch.
const PEN: (i32, i32) = (SCRATCH / 4, SCRATCH * 3 / 4);

/// How one letter fits the tile at one text size: its scale, and its ink
/// box from the pen (x right, y down from the baseline) at that scale.
#[derive(Clone, Copy)]
pub(crate) struct Fit {
    letter: char,
    size: u32,
    scale: f32,
    ink: Rect,
}

impl Painter {
    /// The monogram of `label` on its tile at `at`, as far as `clip` shows.
    pub(crate) fn monogram(&mut self, c: &mut Canvas<'_>, clip: Rect, at: (i32, i32), label: &str) {
        let tile = Rect::new(at.0, at.1, TILE, TILE);
        let Some(shown) = tile.intersect(&clip) else {
            return;
        };
        fill(c, shown, clip, self.palette.hover);
        let Some(first) = label.chars().next() else {
            return;
        };
        let letter = first.to_ascii_uppercase();
        let fit = self.fit(letter);
        if fit.ink.w <= 0 {
            return;
        }
        let x = tile.x + (TILE - fit.ink.w) / 2 - fit.ink.x;
        let y = tile.y + (TILE - fit.ink.h) / 2 - fit.ink.y;
        let mut text = [0u8; 4];
        let text = letter.encode_utf8(&mut text);
        let ink = monogram_ink(&self.palette);
        self.text.draw(
            c,
            g(shown),
            x as f32,
            y,
            TypeStyle::Label,
            fit.scale,
            ink,
            text,
        );
    }

    /// `letter`'s fit at the theme's Label size, measured once.
    fn fit(&mut self, letter: char) -> Fit {
        let size = self.text.style(TypeStyle::Label).size.to_bits();
        if let Some(fit) = self
            .monograms
            .iter()
            .find(|f| f.letter == letter && f.size == size)
        {
            return *fit;
        }
        let whole = self.ink(letter, 1.0);
        let room = (TILE - 2 * INSET) as f32;
        let most = whole.w.max(whole.h).max(1) as f32;
        let scale = (room / most).min(1.0);
        let ink = match scale < 1.0 {
            true => self.ink(letter, scale),
            false => whole,
        };
        let fit = Fit {
            letter,
            size,
            scale,
            ink,
        };
        self.monograms.push(fit);
        fit
    }

    /// `letter`'s ink box at `scale`, from its pen, drawn on a scratch.
    fn ink(&mut self, letter: char, scale: f32) -> Rect {
        let mut scratch = Image::new(SCRATCH, SCRATCH);
        let mut c = scratch.canvas();
        let mut text = [0u8; 4];
        let text = letter.encode_utf8(&mut text);
        let all = gfx2d::Rect::new(0, 0, SCRATCH, SCRATCH);
        let (x, y) = PEN;
        let white = (1 << 24) - 1;
        self.text.draw(
            &mut c,
            all,
            x as f32,
            y,
            TypeStyle::Label,
            scale,
            white,
            text,
        );
        let inked: Vec<(i32, i32)> = (0..SCRATCH)
            .flat_map(|y| (0..SCRATCH).map(move |x| (x, y)))
            .filter(|&(x, y)| scratch.px[(y * SCRATCH + x) as usize] & white != 0)
            .collect();
        let (Some(left), Some(right)) = (
            inked.iter().map(|p| p.0).min(),
            inked.iter().map(|p| p.0).max(),
        ) else {
            return Rect::new(0, 0, 0, 0);
        };
        let top = inked.iter().map(|p| p.1).min().unwrap_or(y);
        let bottom = inked.iter().map(|p| p.1).max().unwrap_or(y);
        Rect::new(left - x, top - y, right - left + 1, bottom - top + 1)
    }
}
