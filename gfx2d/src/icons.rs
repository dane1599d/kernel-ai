//! Kai glyphs: the desktop's and the apps' icons, drawn in code (Kai Desk
//! redesign, PR 4). Each is path geometry on a 24-unit grid with a 2-unit safe
//! margin, stroked 1.5 units (never under 1 px) with round caps and joins, or
//! filled where a dot or a block reads better. The geometry is authored here
//! and licensed with the OS, not traced from another icon set; a shape as
//! plain as a chevron or a cross is kept off the common sets' coordinates.
//!
//! An icon becomes a monochrome coverage [`Mask`] at a pixel size, which the
//! caller tints with a `look` token's colour; [`Icons`] keeps the masks it has
//! made, per (icon, px). The sizes the desk uses are [`SIZES`].

mod glyph;
#[cfg(test)]
mod tests;

use crate::canvas::{Canvas, Rect, blend_row};
use alloc::vec::Vec;
use glyph::{Glyph, Part};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Pixmap, Stroke, Transform};

/// The grid an icon is drawn on, in units.
pub const GRID: f32 = 24.0;
/// The stroke width, in grid units.
pub const STROKE: f32 = 1.5;
/// The pixel sizes the desk draws icons at.
pub const SIZES: [i32; 4] = [16, 20, 24, 32];

/// One Kai glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// Kai's mark: a ring broken at the upper right by a spark dot.
    Kai,
    Terminal,
    Files,
    Notes,
    Editor,
    Monitor,
    Models,
    Settings,
    Search,
    Chat,
    Memory,
    ChevronRight,
    ChevronDown,
    Check,
    Stop,
    Send,
    More,
    Power,
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Icon {
    /// Every glyph, in contact-sheet order.
    pub const ALL: [Icon; 22] = [
        Icon::Kai,
        Icon::Terminal,
        Icon::Files,
        Icon::Notes,
        Icon::Editor,
        Icon::Monitor,
        Icon::Models,
        Icon::Settings,
        Icon::Search,
        Icon::Chat,
        Icon::Memory,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::Check,
        Icon::Stop,
        Icon::Send,
        Icon::More,
        Icon::Power,
        Icon::Minimize,
        Icon::Maximize,
        Icon::Restore,
        Icon::Close,
    ];

    /// The name `uikit`'s `Op::Icon` and the desk use for it.
    pub const fn name(self) -> &'static str {
        match self {
            Icon::Kai => "kai",
            Icon::Terminal => "terminal",
            Icon::Files => "files",
            Icon::Notes => "notes",
            Icon::Editor => "editor",
            Icon::Monitor => "monitor",
            Icon::Models => "models",
            Icon::Settings => "settings",
            Icon::Search => "search",
            Icon::Chat => "chat",
            Icon::Memory => "memory",
            Icon::ChevronRight => "chevron-right",
            Icon::ChevronDown => "chevron-down",
            Icon::Check => "check",
            Icon::Stop => "stop",
            Icon::Send => "send",
            Icon::More => "more",
            Icon::Power => "power",
            Icon::Minimize => "minimize",
            Icon::Maximize => "maximize",
            Icon::Restore => "restore",
            Icon::Close => "close",
        }
    }

    /// The glyph called `name`, if there is one.
    pub fn from_name(name: &str) -> Option<Icon> {
        Icon::ALL.into_iter().find(|i| i.name() == name)
    }

    /// The glyph's parts, in grid units.
    fn parts(self) -> Vec<Part> {
        let mut g = Glyph::default();
        match self {
            Icon::Kai => {
                // A gap from 20 to 70 degrees holds the spark.
                g.arc(12.0, 12.0, 7.5, 70.0, 380.0);
                g.dot(12.0 + 7.5 * COS45, 12.0 - 7.5 * COS45, 2.25);
            }
            Icon::Terminal => {
                g.round_rect(3.0, 5.0, 21.0, 19.0, 2.0);
                g.lines(&[(7.0, 9.5), (9.5, 12.0), (7.0, 14.5)]);
                g.lines(&[(12.0, 15.0), (16.5, 15.0)]);
            }
            Icon::Files => g.folder(),
            Icon::Notes => g.page(),
            Icon::Editor => {
                g.brace(9.0, -1.0);
                g.brace(15.0, 1.0);
            }
            Icon::Monitor => {
                g.round_rect(3.0, 4.0, 21.0, 20.0, 2.0);
                let pulse = [
                    (6.0, 13.0),
                    (9.0, 13.0),
                    (11.0, 9.0),
                    (13.0, 16.0),
                    (15.0, 12.0),
                ];
                g.lines(&pulse);
                g.lines(&[(15.0, 12.0), (18.0, 12.0)]);
            }
            Icon::Models => {
                g.closed(&[(12.0, 4.0), (20.0, 8.0), (12.0, 12.0), (4.0, 8.0)]);
                g.lines(&[(4.0, 12.0), (12.0, 16.0), (20.0, 12.0)]);
                g.lines(&[(4.0, 16.0), (12.0, 20.0), (20.0, 16.0)]);
            }
            Icon::Settings => g.gear(),
            Icon::Search => {
                g.arc(10.5, 10.5, 6.0, 0.0, 360.0);
                g.lines(&[(15.0, 15.0), (20.0, 20.0)]);
            }
            Icon::Chat => g.bubble(),
            Icon::Memory => g.chip(),
            Icon::ChevronRight => g.lines(&[(10.0, 6.0), (16.0, 12.0), (10.0, 18.0)]),
            Icon::ChevronDown => g.lines(&[(6.0, 9.5), (12.0, 15.5), (18.0, 9.5)]),
            Icon::Check => g.lines(&[(5.0, 12.5), (10.0, 17.0), (19.0, 7.0)]),
            Icon::Stop => g.fill_round_rect(7.0, 7.0, 17.0, 17.0, 2.0),
            Icon::Send => {
                g.lines(&[(5.0, 12.0), (19.0, 12.0)]);
                g.lines(&[(13.0, 6.0), (19.0, 12.0), (13.0, 18.0)]);
            }
            Icon::More => {
                for x in [6.0, 12.0, 18.0] {
                    g.dot(x, 12.0, 1.6);
                }
            }
            Icon::Power => {
                g.arc(12.0, 13.0, 7.5, 120.0, 420.0);
                g.lines(&[(12.0, 3.5), (12.0, 11.0)]);
            }
            Icon::Minimize => g.lines(&[(6.0, 12.0), (18.0, 12.0)]),
            Icon::Maximize => g.round_rect(6.0, 6.0, 18.0, 18.0, 1.0),
            Icon::Restore => {
                g.round_rect(5.0, 9.0, 15.0, 19.0, 1.0);
                g.restore_back();
            }
            Icon::Close => {
                g.lines(&[(6.5, 6.5), (17.5, 17.5)]);
                g.lines(&[(17.5, 6.5), (6.5, 17.5)]);
            }
        }
        g.parts
    }
}

const COS45: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// The stroke width at `px`: [`STROKE`] units, never under 1 px.
pub fn stroke_px(px: i32) -> f32 {
    (STROKE * px as f32 / GRID).max(1.0)
}

/// An icon's coverage at one size: `size` x `size` bytes, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    pub size: i32,
    pub coverage: Vec<u8>,
}

impl Mask {
    /// Draws the mask with its top left at (`x`, `y`) in `color`
    /// (`0x00RRGGBB`; each pixel keeps its alpha byte), as far as `clip`.
    pub fn draw(&self, c: &mut Canvas<'_>, x: i32, y: i32, color: u32, clip: Rect) {
        // A mask whose coverage is not `size` squared draws nothing.
        if self.size <= 0 || self.coverage.len() != (self.size * self.size) as usize {
            return;
        }
        let area = Rect::new(x, y, self.size, self.size)
            .intersect(&clip)
            .intersect(&c.bounds());
        if area.is_empty() {
            return;
        }
        let (c0, c1) = (area.x - x, area.right() - x);
        for row in (area.y - y)..(area.bottom() - y) {
            let from = (row * self.size) as usize;
            let cov = &self.coverage[from + c0 as usize..from + c1 as usize];
            let at = (y + row) as usize * c.stride_px as usize + area.x as usize;
            blend_row(&mut c.pixels[at..at + cov.len()], cov, color);
        }
    }
}

/// `icon` at `px` x `px`, drawn `pad` px in from each side of a canvas
/// `px + 2 * pad` wide: the coverage of the whole canvas.
fn raster(icon: Icon, px: i32, pad: i32) -> Vec<u8> {
    let side = (px + 2 * pad).max(1) as u32;
    let Some(mut pixmap) = Pixmap::new(side, side) else {
        // No canvas: blank, but the right size, so a mask still draws safely.
        return alloc::vec![0; (side * side) as usize];
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, 255);
    paint.anti_alias = true;
    let unit = px as f32 / GRID;
    // A stroke an odd number of pixels wide is centred on pixel centres, so a
    // line on a whole grid unit lands on one row of pixels, not two halves.
    let snap = match libm::roundf(stroke_px(px)) as i32 % 2 {
        1 => 0.5,
        _ => 0.0,
    };
    let origin = pad as f32 + snap;
    let at = Transform::from_row(unit, 0.0, 0.0, unit, origin, origin);
    let stroke = Stroke {
        // In grid units, so the transform scales it to `stroke_px`.
        width: stroke_px(px) / unit,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    for part in icon.parts() {
        match part {
            Part::Stroke(path) => pixmap.stroke_path(&path, &paint, &stroke, at, None),
            Part::Fill(path) => pixmap.fill_path(&path, &paint, FillRule::Winding, at, None),
        }
    }
    pixmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[3])
        .collect()
}

/// The largest size a mask is made at: a 32 px icon at 4x. A larger ask is
/// drawn at this size.
pub const MAX_PX: i32 = 128;

/// The most masks [`Icons`] keeps: every glyph at the four desk sizes, with
/// room for a second scale's. Past it the oldest is dropped.
pub const MAX_MASKS: usize = 2 * SIZES.len() * Icon::ALL.len();

/// `icon` as a coverage mask `px` x `px`, `px` held to 1..=[`MAX_PX`].
pub fn render(icon: Icon, px: i32) -> Mask {
    let size = px.clamp(1, MAX_PX);
    Mask {
        size,
        coverage: raster(icon, size, 0),
    }
}

/// The masks made so far, per (icon, px), so each is rasterised once; at most
/// [`MAX_MASKS`], the oldest dropped first.
#[derive(Default)]
pub struct Icons {
    masks: Vec<(Icon, i32, Mask)>,
}

impl Icons {
    /// `icon`'s mask at `px` (held as [`render`] holds it), made the first
    /// time it is asked for.
    pub fn get(&mut self, icon: Icon, px: i32) -> &Mask {
        let px = px.clamp(1, MAX_PX);
        let found = self
            .masks
            .iter()
            .position(|(i, p, _)| *i == icon && *p == px);
        let at = match found {
            Some(at) => at,
            None => {
                if self.masks.len() >= MAX_MASKS {
                    self.masks.remove(0);
                }
                self.masks.push((icon, px, render(icon, px)));
                self.masks.len() - 1
            }
        };
        &self.masks[at].2
    }

    /// How many masks are kept.
    pub fn len(&self) -> usize {
        self.masks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.masks.is_empty()
    }

    /// Draws `icon` at `px` with its top left at (`x`, `y`) in `color`, as far
    /// as `clip`.
    pub fn draw(
        &mut self,
        c: &mut Canvas<'_>,
        (icon, px): (Icon, i32),
        (x, y): (i32, i32),
        color: u32,
        clip: Rect,
    ) {
        self.get(icon, px).draw(c, x, y, color, clip);
    }
}
