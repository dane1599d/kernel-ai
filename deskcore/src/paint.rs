//! Drawing the desk (ADR 0005 §5, §8): the desk, each window's chrome and
//! content, the focused window's shadow, the agent's outline, the top bar, the
//! panel, the ask card and the cursor, in Ember Workbench's dark tokens and type.
//! Pure: the same desk and [`Bar`] give the same pixels, on the host as in
//! `desktop`, so the look is pinned by a host test and by `desk-boot`'s golden.
//!
//! Each window is drawn as a layer (frame, title bar, controls, content), its
//! corners rounded, then put on the frame: copied where opaque and blended at
//! the corners. Only the focused window has a shadow, two nine-slice layers kept
//! inside [`SHADOW_REACH`]; `desktop` grows its damage by that much, so a moved
//! or refocused window takes its shadow with it.

use crate::chrome::{self, Part};
use crate::desk::{Desk, Focus, Window, WindowId};
use crate::geom::{Point, Rect};
use crate::trusted;
use alloc::format;
use gfx2d::shadow::Shadow;
use gfx2d::{Canvas, Fonts, Image, Text};
use look::{DARK, TypeStyle};

/// How far outside a frame its drawing reaches: the shadow fits in the grabs.
pub const SHADOW_REACH: i32 = chrome::GRAB;
/// Space before and after text in a bar.
const PAD: i32 = 12;
/// The varying right side of the top bar: model state and memory.
pub const STATS_W: i32 = 320;
/// A tab's widest.
const TAB_MAX: i32 = 200;
/// The software cursor's box, for its damage.
pub const CURSOR: i32 = 18;

/// What the top bar shows on its right.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    /// A process named `llm` runs.
    pub model: bool,
    pub mem_used_mib: u64,
    pub mem_total_mib: u64,
}

pub struct Painter {
    text: Text,
    near: Shadow,
    far: Shadow,
}

/// `0xFF` alpha on a token.
const fn opaque(rgb: u32) -> u32 {
    (0xFF << 24) | rgb
}

fn g(r: Rect) -> gfx2d::Rect {
    gfx2d::Rect::new(r.x, r.y, r.w, r.h)
}

/// The top bar's part whose text changes while the desk is still.
pub fn stats_area(width: i32) -> Rect {
    Rect::new(width - STATS_W, 0, STATS_W, chrome::TOP_BAR - 1)
}

/// `r` grown by what its drawing reaches outside it, for damage.
pub fn reach(r: Rect) -> Rect {
    r.grow(SHADOW_REACH)
}

/// The software cursor's box with its tip at `at`.
pub fn cursor_box(at: Point) -> Rect {
    Rect::new(at.x, at.y, CURSOR, CURSOR)
}

impl Painter {
    /// None when a font does not parse.
    pub fn new(fonts: Fonts) -> Option<Painter> {
        Some(Painter {
            text: Text::new(fonts, true)?,
            near: Shadow::new(2, 96, 0),
            far: Shadow::new(SHADOW_REACH, 64, 0),
        })
    }

    /// Paints everything inside `clip`. `content` draws window `id`'s pixels
    /// into the canvas given (the window's layer) at the rectangle given, and
    /// says whether it had any; a window with none shows `window`.
    pub fn paint(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        desk: &Desk,
        bar: &Bar,
        content: &mut dyn FnMut(WindowId, &mut Canvas<'_>, Rect) -> bool,
    ) {
        c.fill(g(clip), opaque(DARK.desk));
        for window in desk.windows().iter().filter(|w| !w.minimized) {
            if reach(window.frame).intersect(&clip).is_none() {
                continue;
            }
            let focused = desk.focus() == Focus::Window(window.id);
            if focused {
                self.far.draw(c, g(window.frame), g(clip));
                self.near.draw(c, g(window.frame), g(clip));
            }
            let layer = self.layer(window, focused, content);
            let at = (window.frame.x, window.frame.y);
            c.draw_window(&layer, at, g(clip), chrome::RADIUS, true);
            if desk.agent() == Some(window.id) {
                outline(c, trusted::outline(window.frame), clip);
            }
        }
        if desk.top_bar().intersect(&clip).is_some() {
            self.top_bar(c, clip, desk, bar);
        }
        let panel = desk.panel();
        if panel.intersect(&clip).is_some() {
            fill(c, panel, clip, DARK.window);
            fill(c, Rect::new(panel.x, panel.y, panel.w, 1), clip, DARK.line);
        }
        if let Some(ask) = desk.ask() {
            let (width, height) = (c.width as i32, c.height as i32);
            let card = trusted::card(width, height, &ask);
            fill(c, card, clip, DARK.raised);
            fill(c, Rect::new(card.x, card.y, card.w, 3), clip, DARK.ask);
        }
    }

    /// Window `window`'s frame, title bar, controls and content, its corners
    /// rounded.
    fn layer(
        &mut self,
        window: &Window,
        focused: bool,
        content: &mut dyn FnMut(WindowId, &mut Canvas<'_>, Rect) -> bool,
    ) -> Image {
        let local = Rect::new(0, 0, window.frame.w, window.frame.h);
        let mut layer = Image::new(local.w, local.h);
        let mut c = layer.canvas();
        let line = if focused { DARK.line_strong } else { DARK.line };
        c.fill(g(local), opaque(line));
        let bar = chrome::title_bar(local);
        c.fill(g(bar), opaque(DARK.window));
        let controls = chrome::controls(local);
        let text_end = controls[2].1.x - chrome::CONTROL_GAP;
        let text_clip = Rect::new(bar.x, bar.y, text_end - bar.x, bar.h);
        let (label, title) = match focused {
            true => (DARK.muted, DARK.text),
            false => (DARK.dim, DARK.dim),
        };
        let y = baseline(bar.y, bar.h, TypeStyle::MonoTitle);
        let style = TypeStyle::MonoTitle;
        let x = (bar.x + PAD) as f32;
        let x = x + self.draw_text(&mut c, text_clip, x, y, style, label, &window.label);
        if !window.title.is_empty() {
            let rest = format!("  {}", window.title);
            self.draw_text(&mut c, text_clip, x, y, style, title, &rest);
        }
        for (part, r) in controls {
            icon(&mut c, part, r, if focused { DARK.muted } else { DARK.dim });
        }
        let inside = chrome::content_of(local);
        if !content(window.id, &mut c, inside) {
            c.fill(g(inside), opaque(DARK.window));
        }
        layer.round_corners(chrome::RADIUS);
        layer
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        x: f32,
        y: i32,
        style: TypeStyle,
        color: u32,
        text: &str,
    ) -> f32 {
        self.text.draw(c, g(clip), x, y, style, 1.0, color, text)
    }

    /// The brand, the windows as tabs, and on the right the model and memory.
    fn top_bar(&mut self, c: &mut Canvas<'_>, clip: Rect, desk: &Desk, bar: &Bar) {
        let area = desk.top_bar();
        let Some(clip) = area.intersect(&clip) else {
            return;
        };
        fill(c, area, clip, DARK.window);
        let rule = Rect::new(area.x, area.bottom() - 1, area.w, 1);
        fill(c, rule, clip, DARK.line);
        let h = area.h - 1;
        // The brand: an ember ring and the name, as the console's status bar.
        let r = 5;
        let (cx, cy) = (area.x + PAD + r, area.y + h / 2);
        disc(c, clip, (cx, cy), r as f32, DARK.ai);
        disc(c, clip, (cx, cy), r as f32 / 2.0, DARK.window);
        let y = baseline(area.y, h, TypeStyle::Label);
        let x = (cx + r + 8) as f32;
        let x = x + self.draw_text(c, clip, x, y, TypeStyle::Label, DARK.text, "Kernel AI");
        // Tabs, bottom to top as the windows stack.
        let stats = stats_area(area.w);
        let mut x = x as i32 + 2 * PAD;
        let y = baseline(area.y, h, TypeStyle::MonoTitle);
        for window in desk.windows() {
            let name = if window.title.is_empty() {
                &window.label
            } else {
                &window.title
            };
            let (w, _) = self.text.measure(TypeStyle::MonoTitle, name);
            let tab_w = (w as i32 + 2 * PAD).min(TAB_MAX);
            if x + tab_w > stats.x {
                break;
            }
            let tab = Rect::new(x, area.y + 4, tab_w, h - 8);
            let focused = desk.focus() == Focus::Window(window.id);
            let color = match (focused, window.minimized) {
                (true, _) => {
                    fill(c, tab, clip, DARK.raised);
                    DARK.text
                }
                (false, true) => DARK.dim,
                (false, false) => DARK.muted,
            };
            let inside = tab.intersect(&clip).unwrap_or_default();
            let tx = (x + PAD) as f32;
            self.draw_text(c, inside, tx, y, TypeStyle::MonoTitle, color, name);
            x += tab_w + 4;
        }
        self.stats(c, clip, stats, bar);
    }

    /// The right side, right-aligned in `area`.
    fn stats(&mut self, c: &mut Canvas<'_>, clip: Rect, area: Rect, bar: &Bar) {
        let Some(clip) = area.intersect(&clip) else {
            return;
        };
        let style = TypeStyle::MonoTitle;
        let (dot, model) = match bar.model {
            true => (DARK.ok, "model ready"),
            false => (DARK.dim, "no model"),
        };
        let used = format!("{}", bar.mem_used_mib);
        let total = format!("/{} MiB", bar.mem_total_mib);
        let parts: [(&str, u32); 4] = [
            (model, DARK.text),
            ("   mem ", DARK.dim),
            (&used, DARK.text),
            (&total, DARK.dim),
        ];
        let width: f32 = parts
            .iter()
            .map(|(t, _)| self.text.measure(style, t).0)
            .sum();
        let mut x = (area.right() - PAD) as f32 - width;
        let y = baseline(area.y, area.h, style);
        disc(c, clip, (x as i32 - 10, area.y + area.h / 2), 3.0, dot);
        for (text, color) in parts {
            x += self.draw_text(c, clip, x, y, style, color, text);
        }
    }

    /// The pointer, drawn by `desktop` when the screen has no hardware cursor.
    pub fn cursor(&self, c: &mut Canvas<'_>, clip: Rect, at: Point) {
        let r = cursor_box(at).intersect(&clip).unwrap_or_default();
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                if let Some(color) = arrow(x - at.x, y - at.y) {
                    c.fill(gfx2d::Rect::new(x, y, 1, 1), color);
                }
            }
        }
    }
}

/// The arrow's pixel at `(x, y)` from its tip: `text` inside a `desk` edge,
/// None outside it. The hardware cursor's image is the same arrow.
pub fn arrow(x: i32, y: i32) -> Option<u32> {
    let width = y.min(11);
    if !(0..18).contains(&y) || !(0..=width).contains(&x) {
        return None;
    }
    let edge = x == 0 || x == width || y == 17;
    Some(opaque(if edge { DARK.desk } else { DARK.text }))
}

/// The baseline for `style` centred in a band `h` px tall from `top`: the
/// capitals' height, about 0.72 of the size, in the middle.
fn baseline(top: i32, h: i32, style: TypeStyle) -> i32 {
    let size = look::style(style, false).size;
    top + libm::roundf((h as f32 + 0.72 * size) / 2.0) as i32
}

fn fill(c: &mut Canvas<'_>, r: Rect, clip: Rect, rgb: u32) {
    if let Some(r) = r.intersect(&clip) {
        c.fill(g(r), opaque(rgb));
    }
}

fn outline(c: &mut Canvas<'_>, r: Rect, clip: Rect) {
    let t = trusted::OUTLINE;
    for edge in [
        Rect::new(r.x, r.y, r.w, t),
        Rect::new(r.x, r.bottom() - t, r.w, t),
        Rect::new(r.x, r.y, t, r.h),
        Rect::new(r.right() - t, r.y, t, r.h),
    ] {
        fill(c, edge, clip, DARK.ai);
    }
}

/// `rgb` at coverage `a` (0-1) on one pixel.
fn plot(c: &mut Canvas<'_>, clip: Rect, x: i32, y: i32, rgb: u32, a: f32) {
    if a <= 0.0 || !clip.contains(Point { x, y }) {
        return;
    }
    let a = libm::roundf(a.min(1.0) * 255.0) as u32;
    c.fill_alpha(gfx2d::Rect::new(x, y, 1, 1), rgb, a);
}

/// A filled disc, its edge antialiased.
fn disc(c: &mut Canvas<'_>, clip: Rect, at: (i32, i32), r: f32, rgb: u32) {
    let reach = libm::ceilf(r) as i32 + 1;
    for y in at.1 - reach..=at.1 + reach {
        for x in at.0 - reach..=at.0 + reach {
            let (dx, dy) = ((x - at.0) as f32, (y - at.1) as f32);
            let d = libm::sqrtf(dx * dx + dy * dy);
            plot(c, clip, x, y, rgb, r + 0.5 - d);
        }
    }
}

/// A straight stroke `width` px wide from `a` to `b`, antialiased.
fn stroke(c: &mut Canvas<'_>, clip: Rect, a: (f32, f32), b: (f32, f32), width: f32, rgb: u32) {
    let (x0, x1) = (a.0.min(b.0) - width, a.0.max(b.0) + width);
    let (y0, y1) = (a.1.min(b.1) - width, a.1.max(b.1) + width);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = (dx * dx + dy * dy).max(f32::EPSILON);
    for y in libm::floorf(y0) as i32..=libm::ceilf(y1) as i32 {
        for x in libm::floorf(x0) as i32..=libm::ceilf(x1) as i32 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0);
            let (qx, qy) = (a.0 + t * dx - px, a.1 + t * dy - py);
            let d = libm::sqrtf(qx * qx + qy * qy);
            plot(c, clip, x, y, rgb, width / 2.0 + 0.5 - d);
        }
    }
}

/// A control's Lucide icon (`minus`, `square`, `x`) at 16 px, centred in `r`.
fn icon(c: &mut Canvas<'_>, part: Part, r: Rect, rgb: u32) {
    // Lucide draws on a 24 px grid with 2 px strokes.
    let k = 16.0 / 24.0;
    let (ox, oy) = ((r.x + (r.w - 16) / 2) as f32, (r.y + (r.h - 16) / 2) as f32);
    let p = |x: f32, y: f32| (ox + x * k, oy + y * k);
    let w = 2.0 * k;
    let clip = r;
    match part {
        Part::Minimize => stroke(c, clip, p(5.0, 12.0), p(19.0, 12.0), w, rgb),
        Part::Maximize => {
            let corners = [p(3.0, 3.0), p(21.0, 3.0), p(21.0, 21.0), p(3.0, 21.0)];
            for i in 0..4 {
                stroke(c, clip, corners[i], corners[(i + 1) % 4], w, rgb);
            }
        }
        Part::Close => {
            stroke(c, clip, p(18.0, 6.0), p(6.0, 18.0), w, rgb);
            stroke(c, clip, p(6.0, 6.0), p(18.0, 18.0), w, rgb);
        }
        _ => {}
    }
}
