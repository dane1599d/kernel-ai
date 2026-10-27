//! Text (ADR 0005 §5). harfrust shapes a run, skrifa draws each glyph's outline,
//! tiny-skia fills it to 8-bit coverage, and the atlas keeps it, keyed by (font,
//! glyph, size in 1/64 px, x phase in quarters, weight). Light vertical hinting
//! at scale 1, none above. Greyscale only: coverage goes through the theme's
//! table, dark `c' = 255 (c / 255)^0.8`, light the identity.
//!
//! Printable-ASCII monospace text takes the cell path: each character's glyph and
//! advance are shaped once per face, and a run is laid out cell by cell, as a
//! terminal draws it (no ligatures, no kerning). S05-1 measured it: the warm
//! 160x45 page went from 13.61 to 3.19 ms at p99 under KVM.

use crate::canvas::{Canvas, Rect, blend_row};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use harfrust::{Buffer, ShaperFont};
use look::{Family, TypeStyle};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, HintingInstance, OutlinePen, SmoothMode, Target};
use skrifa::raw::TableProvider;
use skrifa::raw::types::GlyphId;
use skrifa::{FontRef, MetadataProvider};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

/// The font files ADR 0005 §8 names: Inter 4.1 and JetBrains Mono 2.304, both
/// variable (`wght`), from the boot bundle. The legible setting's Atkinson
/// Hyperlegible arrives with P4's themes; until then `legible` sizes Inter and
/// JetBrains Mono 1 px larger.
#[derive(Clone, Copy)]
pub struct Fonts {
    pub sans: &'static [u8],
    pub mono: &'static [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Face {
    Sans,
    Mono,
}

fn face_of(family: Family) -> Face {
    match family {
        Family::Mono | Family::LegibleMono => Face::Mono,
        Family::Sans | Family::LegibleSans => Face::Sans,
    }
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
    face: Face,
    glyph: u32,
    size64: u32,
    phase: u8,
    weight: u16,
}

struct Loaded {
    skrifa: FontRef<'static>,
    shaper: ShaperFont<'static, 'static>,
    upem: f32,
    /// Printable ASCII's glyph and advance (font units), shaped alone once.
    ascii: [(u32, i32); 128],
}

fn load(data: &'static [u8]) -> Option<Loaded> {
    let skrifa = FontRef::new(data).ok()?;
    let upem = f32::from(skrifa.head().ok()?.units_per_em());
    let font: &'static harfrust::Font = Box::leak(Box::new(harfrust::Font::new(data.to_vec(), 0)?));
    let shaper: ShaperFont<'static, 'static> = ShaperFont::new(font);
    let mut ascii = [(0u32, 0i32); 128];
    for (c, slot) in ascii.iter_mut().enumerate().skip(32).take(95) {
        let mut buffer = Buffer::new();
        buffer.push_str(core::str::from_utf8(&[c as u8]).unwrap_or(" "));
        buffer.guess_segment_properties();
        if harfrust::shape(&shaper, &mut buffer, harfrust::ShapeOptions::new()).is_ok()
            && let (Some(info), Some(pos)) = (
                buffer.glyph_infos().first(),
                buffer.glyph_positions().first(),
            )
        {
            *slot = (info.glyph_id, pos.x_advance);
        }
    }
    Some(Loaded {
        skrifa,
        shaper,
        upem,
        ascii,
    })
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

/// A run's style resolved for drawing: face, size in px, weight.
#[derive(Clone, Copy, Debug)]
struct Run {
    face: Face,
    size: f32,
    weight: u16,
}

pub struct Text {
    faces: [Loaded; 2],
    atlas: Vec<Glyph>,
    index: BTreeMap<Key, u32>,
    hinting: BTreeMap<(Face, u32, u16), HintingInstance>,
    table: [u8; 256],
    /// The legible setting (ADR 0005 §8).
    pub legible: bool,
    /// Glyphs rasterised so far, for the atlas's figures.
    pub misses: u64,
}

impl Text {
    /// None when a font does not parse.
    pub fn new(fonts: Fonts, dark: bool) -> Option<Text> {
        Some(Text {
            faces: [load(fonts.sans)?, load(fonts.mono)?],
            atlas: Vec::new(),
            index: BTreeMap::new(),
            hinting: BTreeMap::new(),
            table: table(if dark { 0.8 } else { 1.0 }),
            legible: false,
            misses: 0,
        })
    }

    /// The theme's coverage table; the atlas starts again.
    pub fn set_dark(&mut self, dark: bool) {
        self.table = table(if dark { 0.8 } else { 1.0 });
        self.clear();
    }

    /// Empties the atlas.
    pub fn clear(&mut self) {
        self.atlas.clear();
        self.index.clear();
    }

    fn run(&self, style: TypeStyle, scale: f32) -> (Run, f32) {
        let t = look::style(style, self.legible);
        let run = Run {
            face: face_of(t.family),
            size: t.size * scale,
            weight: t.weight,
        };
        (run, t.line)
    }

    /// Each glyph and its pen position, in px from the run's start, and the run's
    /// advance.
    fn shape(&self, run: Run, text: &str) -> (Vec<(u32, f32)>, f32) {
        let face = &self.faces[run.face as usize];
        let scale = run.size / face.upem;
        if run.face == Face::Mono && text.bytes().all(|b| (32..127).contains(&b)) {
            let mut x = 0.0;
            let out = text
                .bytes()
                .map(|b| {
                    let (glyph, advance) = face.ascii[usize::from(b)];
                    let at = x;
                    x += advance as f32 * scale;
                    (glyph, at)
                })
                .collect();
            return (out, x);
        }
        let mut buffer = Buffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let mut out = Vec::new();
        let mut x = 0.0;
        if harfrust::shape(&face.shaper, &mut buffer, harfrust::ShapeOptions::new()).is_ok() {
            for (info, pos) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
                out.push((info.glyph_id, x + pos.x_offset as f32 * scale));
                x += pos.x_advance as f32 * scale;
            }
        }
        (out, x)
    }

    /// `text`'s advance and line height in logical px, for layout (uikit's
    /// `Measure`).
    pub fn measure(&self, style: TypeStyle, text: &str) -> (f32, f32) {
        let (run, line) = self.run(style, 1.0);
        (self.shape(run, text).1, line)
    }

    fn rasterize(&mut self, key: Key, hint: bool) -> Glyph {
        self.misses += 1;
        let empty = |left, top| Glyph {
            left,
            top,
            w: 0,
            h: 0,
            coverage: Vec::new(),
        };
        let skrifa = &self.faces[key.face as usize].skrifa;
        let outlines = skrifa.outline_glyphs();
        let size = Size::new(key.size64 as f32 / 64.0);
        let location = skrifa.axes().location([("wght", f32::from(key.weight))]);
        let mut path = PathBuilder::new();
        if let Some(glyph) = outlines.get(GlyphId::new(key.glyph)) {
            let mut pen = Pen {
                path: &mut path,
                dx: f32::from(key.phase) / 4.0,
            };
            let hinted = hint && {
                let k = (key.face, key.size64, key.weight);
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
                self.hinting
                    .get(&k)
                    .is_some_and(|h| glyph.draw(DrawSettings::hinted(h, false), &mut pen).is_ok())
            };
            if !hinted {
                let _ = glyph.draw(
                    DrawSettings::unhinted(size, LocationRef::from(&location)),
                    &mut pen,
                );
            }
        }
        let Some(path) = path.finish() else {
            return empty(0, 0);
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
            return empty(left, top);
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

    /// Draws `text` in `style` at `scale`, its baseline at `y` px, from `x` px,
    /// into `c` as far as `clip`, in `color` (`0x00RRGGBB`; each pixel keeps its
    /// alpha byte); returns the advance in px.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        x: f32,
        y: i32,
        style: TypeStyle,
        scale: f32,
        color: u32,
        text: &str,
    ) -> f32 {
        let (run, _) = self.run(style, scale);
        let (glyphs, advance) = self.shape(run, text);
        let size64 = libm::roundf(run.size * 64.0) as u32;
        let hint = libm::fabsf(scale - 1.0) < 1e-3;
        let clip = clip.intersect(&c.bounds());
        // This run's glyphs by (glyph id, phase), for ids under MEMO: a line
        // repeats a few dozen glyphs, so the atlas is searched once for each.
        const MEMO: usize = 1024;
        let mut memo = [u32::MAX; MEMO * 4];
        for (glyph, at) in glyphs {
            let pen = x + at;
            let whole = libm::floorf(pen);
            let phase = (libm::floorf((pen - whole) * 4.0) as u8).min(3);
            let key = Key {
                face: run.face,
                glyph,
                size64,
                phase,
                weight: run.weight,
            };
            let slot = glyph as usize * 4 + usize::from(phase);
            let i = match memo.get(slot).copied() {
                Some(i) if i != u32::MAX => i,
                _ => {
                    let i = match self.index.get(&key) {
                        Some(&i) => i,
                        None => {
                            let g = self.rasterize(key, hint);
                            self.atlas.push(g);
                            let i = (self.atlas.len() - 1) as u32;
                            self.index.insert(key, i);
                            i
                        }
                    };
                    if let Some(m) = memo.get_mut(slot) {
                        *m = i;
                    }
                    i
                }
            };
            let g = &self.atlas[i as usize];
            let (gx, gy) = (whole as i32 + g.left, y + g.top);
            let (c0, c1) = ((clip.x - gx).max(0), (clip.right() - gx).min(g.w));
            let (r0, r1) = ((clip.y - gy).max(0), (clip.bottom() - gy).min(g.h));
            if c0 >= c1 || r0 >= r1 {
                continue;
            }
            for row in r0..r1 {
                let cov = &g.coverage[(row * g.w + c0) as usize..(row * g.w + c1) as usize];
                let at = (gy + row) as usize * c.stride_px as usize + (gx + c0) as usize;
                blend_row(&mut c.pixels[at..at + cov.len()], cov, color);
            }
        }
        advance
    }
}

#[cfg(test)]
mod tests;
