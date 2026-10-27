//! Text: harfrust shapes a run, skrifa draws each glyph's outline, tiny-skia
//! fills it to 8-bit coverage, and the atlas keeps it, keyed by (font, glyph,
//! size in 1/64 px, x phase in quarters, weight). Light vertical hinting at scale
//! 1 and none above (ADR 0005 §5). Greyscale only. Coverage goes through the
//! theme's table: dark c' = 255 (c / 255)^0.8, light the identity.

use crate::frame::{Frame, tint};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use harfrust::{Buffer, ShaperFont};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, HintingInstance, OutlinePen, SmoothMode, Target};
use skrifa::raw::TableProvider;
use skrifa::raw::types::GlyphId;
use skrifa::{FontRef, MetadataProvider};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Sans,
    Mono,
}

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub family: Family,
    /// Pixels (logical px times the scale).
    pub size: f32,
    /// The `wght` axis: 400, 500, 600 or 700.
    pub weight: u16,
}

#[derive(Clone, Debug)]
struct Glyph {
    left: i32,
    top: i32,
    w: i32,
    h: i32,
    coverage: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    family: Family,
    glyph: u32,
    size64: u32,
    phase: u8,
    weight: u16,
}

struct Face {
    skrifa: FontRef<'static>,
    shaper: ShaperFont<'static, 'static>,
    upem: f32,
}

pub struct Text {
    faces: [Face; 2],
    atlas: BTreeMap<Key, Glyph>,
    hinting: BTreeMap<(Family, u32, u16), HintingInstance>,
    table: [u8; 256],
    /// Whether outlines are hinted: scale 1 only.
    pub hint: bool,
    pub misses: u64,
}

fn face(data: &'static [u8]) -> Face {
    let skrifa = FontRef::new(data).expect("a font");
    let upem = f32::from(skrifa.head().map(|h| h.units_per_em()).unwrap_or(1000));
    let font: &'static harfrust::Font = Box::leak(Box::new(
        harfrust::Font::new(data.to_vec(), 0).expect("a font"),
    ));
    let shaper: ShaperFont<'static, 'static> = ShaperFont::new(font);
    Face {
        skrifa,
        shaper,
        upem,
    }
}

/// The coverage table: `255 (c / 255)^gamma`.
fn table(gamma: f32) -> [u8; 256] {
    let mut t = [0u8; 256];
    for (c, out) in t.iter_mut().enumerate() {
        let v = libm::powf(c as f32 / 255.0, gamma) * 255.0 + 0.5;
        *out = v.clamp(0.0, 255.0) as u8;
    }
    t
}

struct Pen<'a> {
    path: &'a mut PathBuilder,
    dx: f32,
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(x + self.dx, -y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(x + self.dx, -y);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.quad_to(cx + self.dx, -cy, x + self.dx, -y);
    }
    fn curve_to(&mut self, a: f32, b: f32, c: f32, d: f32, x: f32, y: f32) {
        self.path
            .cubic_to(a + self.dx, -b, c + self.dx, -d, x + self.dx, -y);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

impl Text {
    pub fn new(sans: &'static [u8], mono: &'static [u8], dark: bool) -> Text {
        Text {
            faces: [face(sans), face(mono)],
            atlas: BTreeMap::new(),
            hinting: BTreeMap::new(),
            table: table(if dark { 0.8 } else { 1.0 }),
            hint: true,
            misses: 0,
        }
    }

    /// Empties the atlas, for a cold measurement.
    pub fn clear(&mut self) {
        self.atlas.clear();
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.table = table(if dark { 0.8 } else { 1.0 });
        self.atlas.clear();
    }

    fn face(&self, f: Family) -> &Face {
        &self.faces[f as usize]
    }

    /// Each glyph and its pen position, in px from the run's start.
    pub fn shape(&self, style: Style, text: &str) -> (Vec<(u32, f32)>, f32) {
        let face = self.face(style.family);
        let mut buffer = Buffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let shaped = harfrust::shape(&face.shaper, &mut buffer, harfrust::ShapeOptions::new());
        let scale = style.size / face.upem;
        let mut out = Vec::new();
        let mut x = 0.0;
        if shaped.is_ok() {
            for (info, pos) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
                out.push((info.glyph_id, x + pos.x_offset as f32 * scale));
                x += pos.x_advance as f32 * scale;
            }
        }
        (out, x)
    }

    fn rasterize(&mut self, key: Key, style: Style) -> Glyph {
        self.misses += 1;
        let skrifa = &self.faces[key.family as usize].skrifa;
        let outlines = skrifa.outline_glyphs();
        let size = Size::new(key.size64 as f32 / 64.0);
        let axes = skrifa.axes();
        let location = axes.location([("wght", f32::from(key.weight))]);
        let mut path = PathBuilder::new();
        let dx = f32::from(key.phase) / 4.0;
        if let Some(glyph) = outlines.get(GlyphId::new(key.glyph)) {
            let mut pen = Pen {
                path: &mut path,
                dx,
            };
            let hinted = self.hint && {
                let k = (key.family, key.size64, key.weight);
                if let alloc::collections::btree_map::Entry::Vacant(slot) = self.hinting.entry(k) {
                    let target = Target::Smooth {
                        mode: SmoothMode::Light,
                        symmetric_rendering: true,
                        preserve_linear_metrics: true,
                    };
                    if let Ok(h) = HintingInstance::new(&outlines, size, &location, target) {
                        slot.insert(h);
                    }
                }
                match self.hinting.get(&k) {
                    Some(h) => glyph.draw(DrawSettings::hinted(h, false), &mut pen).is_ok(),
                    None => false,
                }
            };
            if !hinted {
                let _ = glyph.draw(
                    DrawSettings::unhinted(size, LocationRef::from(&location)),
                    &mut pen,
                );
            }
        }
        let _ = style;
        let Some(path) = path.finish() else {
            return Glyph {
                left: 0,
                top: 0,
                w: 0,
                h: 0,
                coverage: Vec::new(),
            };
        };
        let b = path.bounds();
        let (left, top) = (
            libm::floorf(b.left()) as i32 - 1,
            libm::floorf(b.top()) as i32 - 1,
        );
        let (w, h) = (
            libm::ceilf(b.right()) as i32 - left + 1,
            libm::ceilf(b.bottom()) as i32 - top + 1,
        );
        let Some(mut pixmap) = Pixmap::new(w.max(1) as u32, h.max(1) as u32) else {
            return Glyph {
                left,
                top,
                w: 0,
                h: 0,
                coverage: Vec::new(),
            };
        };
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::from_translate(-left as f32, -top as f32),
            None,
        );
        let coverage = pixmap
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| self.table[usize::from(p[3])])
            .collect();
        Glyph {
            left,
            top,
            w,
            h,
            coverage,
        }
    }

    /// Draws `text` with its baseline at `y` from `x` into `f`, clipped to `clip`;
    /// returns the advance. Pixels keep their alpha byte, so a layer stays opaque.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        f: &mut Frame,
        clip: crate::frame::Rect,
        x: f32,
        y: i32,
        style: Style,
        color: u32,
        text: &str,
    ) -> f32 {
        let (glyphs, advance) = self.shape(style, text);
        let size64 = libm::roundf(style.size * 64.0) as u32;
        let clip = clip.intersect(&f.bounds());
        for (glyph, at) in glyphs {
            let pen = x + at;
            let whole = libm::floorf(pen);
            let phase = (libm::floorf((pen - whole) * 4.0) as u8).min(3);
            let key = Key {
                family: style.family,
                glyph,
                size64,
                phase,
                weight: style.weight,
            };
            if !self.atlas.contains_key(&key) {
                let g = self.rasterize(key, style);
                self.atlas.insert(key, g);
            }
            let g = &self.atlas[&key];
            let (gx, gy) = (whole as i32 + g.left, y + g.top);
            for row in 0..g.h {
                let py = gy + row;
                if py < clip.y || py >= clip.bottom() {
                    continue;
                }
                for col in 0..g.w {
                    let px = gx + col;
                    if px < clip.x || px >= clip.right() {
                        continue;
                    }
                    let a = u32::from(g.coverage[(row * g.w + col) as usize]);
                    if a == 0 {
                        continue;
                    }
                    let p = &mut f.px[(py * f.w + px) as usize];
                    *p = tint(color, a, *p & 0x00FF_FFFF) | (*p & 0xFF00_0000);
                }
            }
        }
        advance
    }
}
