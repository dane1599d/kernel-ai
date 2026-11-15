//! Drawing the desk (ADR 0005 §5, §8): the desk, each window's chrome and
//! content, the focused window's shadow, the agent's outline, the top bar, the
//! panel, the Apps sheet, the ask card and the cursor, in the theme's tokens and
//! type (Ember Workbench's dark one until [`Painter::set_theme`]). The card is
//! drawn last but the cursor, so nothing else covers it (ADR 0005 §7).
//! Pure: the same [`Scene`] gives the same pixels, on the host as in `desktop`,
//! so the look is pinned by a host test and by `desk-boot`'s golden. The panel
//! is drawn in `paint_panel.rs`, and its history on the desk in
//! `paint_chat.rs`.
//!
//! Each window is drawn as a layer (frame, title bar, controls, content), its
//! corners rounded, then put on the frame: copied where opaque and blended at
//! the corners. Only the focused window has a shadow, two nine-slice layers kept
//! inside [`SHADOW_REACH`]; `desktop` grows its damage by that much, so a moved
//! or refocused window takes its shadow with it. A window being dragged has none
//! until it is let go ([`Desk::dragged`]): ADR 0005's fail rule, so a drag frame
//! under load fits its budget.

use crate::askcard::Card;
use crate::chrome::{self, Part};
use crate::desk::{Desk, Focus, Window, WindowId};
use crate::geom::{Point, Rect};
use crate::panel::Panel;
use crate::trusted;
use alloc::format;
use gfx2d::icons::Icon;
use gfx2d::shadow::Shadow;
use gfx2d::{Canvas, Fonts, Image, Text};
use look::theme::Theme;
use look::{DARK, Palette, TypeStyle};

/// How far outside a frame its drawing reaches: the focused window's shadow
/// (`look::metric::SHADOW_REACH`), wider than its grabs.
pub const SHADOW_REACH: i32 = look::metric::SHADOW_REACH as i32;
/// The app icon's side in a title bar (`look::metric::ICON_TITLE`).
const TITLE_ICON: i32 = look::metric::ICON_TITLE as i32;
/// A window control's glyph side.
const CONTROL_ICON: i32 = 16;
/// Between the icon, the kernel label and the title.
const TITLE_GAP: i32 = 8;
/// The high-contrast hover cue's frame: its own 1 px, not the agent
/// outline's ([`trusted::OUTLINE`]).
const HOVER_FRAME: i32 = 1;
/// Space before and after text in a bar.
pub(crate) const PAD: i32 = 12;
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
    /// The shell said the model is not warm yet (`Panel::getting_ready`).
    pub warming: bool,
    pub mem_used_mib: u64,
    pub mem_total_mib: u64,
}

/// Everything a frame shows besides the windows' own pixels.
pub struct Scene<'a> {
    pub desk: &'a Desk,
    pub bar: &'a Bar,
    pub panel: &'a Panel,
    /// The panel's history, drawn in the Kai pane.
    pub chat: &'a crate::chat::Chat,
    /// The Apps sheet, while it is up: over the windows and the panel, under
    /// the ask card.
    pub sheet: Option<&'a crate::catalog::Sheet>,
    /// The text of the ask card up now, as the kernel posted it.
    pub ask: Option<&'a Card>,
    /// The clock, for the panel's "thinking 4s".
    pub now_ms: u64,
}

pub struct Painter {
    pub(crate) text: Text,
    /// The theme's colours.
    pub(crate) palette: Palette,
    pub(crate) near: Shadow,
    pub(crate) far: Shadow,
    /// Our glyphs, by icon and size ([`gfx2d::icons`]).
    pub(crate) icons: gfx2d::icons::Icons,
    /// The history's entries laid out, by entry and width
    /// ([`crate::paint_chat`]): what a repaint draws without wrapping again.
    pub(crate) chat_laid: alloc::vec::Vec<crate::paint_chat::Laid>,
    /// Each monogram letter's fit to its tile, by text size
    /// ([`crate::monogram`]).
    pub(crate) monograms: alloc::vec::Vec<crate::monogram::Fit>,
}

/// `0xFF` alpha on a token.
pub(crate) const fn opaque(rgb: u32) -> u32 {
    (0xFF << 24) | rgb
}

pub(crate) fn g(r: Rect) -> gfx2d::Rect {
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
            text: Text::new(fonts, DARK.dark)?,
            palette: DARK,
            near: Shadow::new(2, 96, 0),
            far: Shadow::new(SHADOW_REACH, 64, 0),
            icons: gfx2d::icons::Icons::default(),
            chat_laid: alloc::vec::Vec::new(),
            monograms: alloc::vec::Vec::new(),
        })
    }

    /// Paints in `theme` from now on; the caller repaints the whole screen.
    pub fn set_theme(&mut self, theme: &Theme) {
        self.palette = theme.palette();
        self.text.set_theme(theme);
        // A theme may change the type's sizes, so the lines' breaks.
        self.chat_laid.clear();
        self.monograms.clear();
    }

    /// The theme's colours, for what `desktop` draws itself (the cursor).
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// What a window shows where its client gave no pixels yet: the theme's
    /// window colour, opaque.
    pub fn content_fallback(&self) -> u32 {
        opaque(self.palette.window)
    }

    /// Paints everything inside `clip`. `content` draws window `id`'s pixels
    /// into the canvas given (the window's layer) at the rectangle given, and
    /// says whether it had any; a window with none shows `window`.
    pub fn paint(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        scene: &Scene<'_>,
        content: &mut dyn FnMut(WindowId, &mut Canvas<'_>, Rect) -> bool,
    ) {
        let (desk, bar) = (scene.desk, scene.bar);
        c.fill(g(clip), opaque(self.palette.desk));
        for window in desk.windows().iter().filter(|w| !w.minimized) {
            if reach(window.frame).intersect(&clip).is_none() {
                continue;
            }
            let focused = desk.focus() == Focus::Window(window.id);
            if focused && desk.dragged() != Some(window.id) {
                self.far.draw(c, g(window.frame), g(clip));
                self.near.draw(c, g(window.frame), g(clip));
            }
            let hovered = desk
                .hovered()
                .filter(|(id, _)| *id == window.id)
                .map(|(_, part)| part);
            let layer = self.layer(window, focused, hovered, content);
            let at = (window.frame.x, window.frame.y);
            let radius = chrome::radius(window.maximized());
            c.draw_window(&layer, at, g(clip), radius, true);
            if desk.agent() == Some(window.id) {
                outline(c, trusted::outline(window.frame), clip, self.palette.ai);
            }
        }
        if desk.top_bar().intersect(&clip).is_some() {
            self.top_bar(c, clip, desk, bar);
        }
        // The Kai pane over the windows and the top bar (ADR 0005 §7).
        self.pane(c, clip, scene);
        if let Some(sheet) = scene.sheet {
            let (width, height) = (c.width as i32, c.height as i32);
            let area = crate::paint_catalog::sheet(width, height, sheet.apps().len());
            self.catalog(c, clip, area, sheet);
        }
        // The card last: nothing drawn after it but the cursor.
        if let Some(ask) = desk.ask() {
            match scene.ask {
                Some(card) if card.id == ask.id => {
                    self.ask_card(c, clip, &ask, card, scene.now_ms);
                }
                // A card with no text from the kernel shows none.
                _ => {
                    let (width, height) = (c.width as i32, c.height as i32);
                    let card = trusted::card(width, height, &ask);
                    fill(c, card, clip, self.palette.raised);
                    fill(
                        c,
                        Rect::new(card.x, card.y, card.w, 3),
                        clip,
                        self.palette.ask,
                    );
                }
            }
        }
    }

    /// Window `window`'s frame, title bar, controls and content, its corners
    /// rounded unless it is maximized; `hovered` is the control under the
    /// pointer.
    fn layer(
        &mut self,
        window: &Window,
        focused: bool,
        hovered: Option<Part>,
        content: &mut dyn FnMut(WindowId, &mut Canvas<'_>, Rect) -> bool,
    ) -> Image {
        let local = Rect::new(0, 0, window.frame.w, window.frame.h);
        let mut layer = Image::new(local.w, local.h);
        let mut c = layer.canvas();
        let line = if focused {
            self.palette.line_strong
        } else {
            self.palette.line
        };
        c.fill(g(local), opaque(line));
        let bar = chrome::title_bar(local);
        c.fill(g(bar), opaque(self.palette.window));
        let controls = chrome::controls(local);
        self.title_row(&mut c, window, focused, bar, controls[2].1.x);
        for (part, r) in controls {
            let shown = Shown {
                focused,
                hovered: hovered == Some(part),
                maximized: window.maximized(),
            };
            self.control(&mut c, part, r, shown);
        }
        let inside = chrome::content_of(local);
        if !content(window.id, &mut c, inside) {
            c.fill(g(inside), opaque(self.palette.window));
        }
        layer.round_corners(chrome::radius(window.maximized()));
        layer
    }

    /// The title row (ADR 0005 §7): the app's icon, the kernel's label for
    /// the process in mono, then the app's own title, cut at its end before
    /// `text_end`. The title starts only after the whole label, in a clip of
    /// its own, so no pixel of a title ever shows without the label before
    /// it, and its marks cannot reach back over the label. In a bar too
    /// narrow for both (a frame under about 250 px) the label is cut at its
    /// end and the title not drawn; at the narrowest, 3 controls fill the bar
    /// and neither shows.
    fn title_row(
        &mut self,
        c: &mut Canvas<'_>,
        window: &Window,
        focused: bool,
        bar: Rect,
        text_end: i32,
    ) {
        let text = Rect::new(bar.x, bar.y, (text_end - bar.x).max(0), bar.h);
        let (title_rgb, icon_rgb) = match focused {
            true => (self.palette.text, self.palette.muted),
            false => (self.palette.dim, self.palette.dim),
        };
        let x = bar.x + PAD;
        let icon_y = bar.y + (bar.h - TITLE_ICON) / 2;
        match app_icon(&window.label) {
            Some(icon) => {
                let at = (x, icon_y);
                self.icons
                    .draw(c, (icon, TITLE_ICON), at, icon_rgb, g(text));
            }
            None => self.monogram(c, text, (x, icon_y), &window.label),
        }
        let x = (x + TITLE_ICON + TITLE_GAP) as f32;
        let y = self.baseline(bar.y, bar.h, TypeStyle::MonoTitle);
        let dim = self.palette.dim;
        let advance = self.draw_text(c, text, x, y, TypeStyle::MonoTitle, dim, &window.label);
        if window.title.is_empty() || window.title == window.label {
            return;
        }
        let y = self.baseline(bar.y, bar.h, TypeStyle::Label);
        let x = x + advance + TITLE_GAP as f32;
        let from = x as i32;
        let clip = Rect::new(from, bar.y, (text_end - from).max(0), bar.h);
        self.draw_text(c, clip, x, y, TypeStyle::Label, title_rgb, &window.title);
    }

    /// One title-bar control: its glyph, on a `hover` fill under the pointer,
    /// close's on `error` with the glyph in `window`.
    fn control(&mut self, c: &mut Canvas<'_>, part: Part, r: Rect, shown: Shown) {
        let rest = match shown.focused {
            true => self.palette.muted,
            false => self.palette.dim,
        };
        let glyph = match (shown.hovered, part) {
            (false, _) => rest,
            (true, Part::Close) => {
                c.fill(g(r), opaque(self.palette.error));
                self.palette.window
            }
            // High contrast: hover is the ground, so a 1 px frame shows it.
            (true, _) if self.palette.hover == self.palette.window => {
                frame(c, r, r, self.palette.focus, HOVER_FRAME);
                hover_ink(&self.palette)
            }
            (true, _) => {
                c.fill(g(r), opaque(self.palette.hover));
                hover_ink(&self.palette)
            }
        };
        let icon = match part {
            Part::Minimize => Icon::Minimize,
            Part::Maximize if shown.maximized => Icon::Restore,
            Part::Maximize => Icon::Maximize,
            _ => Icon::Close,
        };
        let at = (
            r.x + (r.w - CONTROL_ICON) / 2,
            r.y + (r.h - CONTROL_ICON) / 2,
        );
        self.icons.draw(c, (icon, CONTROL_ICON), at, glyph, g(r));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_text(
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
        fill(c, area, clip, self.palette.window);
        let rule = Rect::new(area.x, area.bottom() - 1, area.w, 1);
        fill(c, rule, clip, self.palette.line);
        let h = area.h - 1;
        // The brand: an ember ring and the name, as the console's status bar.
        let r = 5;
        let (cx, cy) = (area.x + PAD + r, area.y + h / 2);
        disc(c, clip, (cx, cy), r as f32, self.palette.ai);
        disc(c, clip, (cx, cy), r as f32 / 2.0, self.palette.window);
        let y = self.baseline(area.y, h, TypeStyle::Label);
        let x = (cx + r + 8) as f32;
        self.draw_text(
            c,
            clip,
            x,
            y,
            TypeStyle::Label,
            self.palette.text,
            "Kernel AI",
        );
        // Tabs, bottom to top as the windows stack.
        let stats = stats_area(area.w);
        let y = self.baseline(area.y, h, TypeStyle::MonoTitle);
        for (id, tab) in self.tabs(desk) {
            let Some(window) = desk.window(id) else {
                continue;
            };
            let name = if window.title.is_empty() {
                &window.label
            } else {
                &window.title
            };
            let x = tab.x;
            let focused = desk.focus() == Focus::Window(window.id);
            let color = match (focused, window.minimized) {
                (true, _) => {
                    fill(c, tab, clip, self.palette.raised);
                    self.palette.text
                }
                (false, true) => self.palette.dim,
                (false, false) => self.palette.muted,
            };
            let inside = tab.intersect(&clip).unwrap_or_default();
            let tx = (x + PAD) as f32;
            self.draw_text(c, inside, tx, y, TypeStyle::MonoTitle, color, name);
        }
        self.stats(c, clip, stats, bar);
    }

    /// The top bar's tabs: one a window, bottom to top as they stack, as many
    /// as fit before the stats. What the bar draws, and what a press on it
    /// hits (`Desk::set_tabs`).
    pub fn tabs(&self, desk: &Desk) -> alloc::vec::Vec<(crate::desk::WindowId, Rect)> {
        let area = desk.top_bar();
        let h = area.h - 1;
        let r = 5;
        let brand = self.text.measure(TypeStyle::Label, "Kernel AI").0 as i32;
        let mut x = area.x + PAD + 2 * r + 8 + brand + 2 * PAD;
        let stats = stats_area(area.w);
        let mut tabs = alloc::vec::Vec::new();
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
            tabs.push((window.id, Rect::new(x, area.y + 4, tab_w, h - 8)));
            x += tab_w + 4;
        }
        tabs
    }

    /// The right side, right-aligned in `area`.
    fn stats(&mut self, c: &mut Canvas<'_>, clip: Rect, area: Rect, bar: &Bar) {
        let Some(clip) = area.intersect(&clip) else {
            return;
        };
        let style = TypeStyle::MonoTitle;
        let (dot, model) = match (bar.model, bar.warming) {
            (true, false) => (self.palette.ok, "model ready"),
            (true, true) => (self.palette.dim, "getting ready"),
            (false, _) => (self.palette.dim, "no model"),
        };
        let used = format!("{}", bar.mem_used_mib);
        let total = format!("/{} MiB", bar.mem_total_mib);
        let parts: [(&str, u32); 4] = [
            (model, self.palette.text),
            ("   mem ", self.palette.dim),
            (&used, self.palette.text),
            (&total, self.palette.dim),
        ];
        let width: f32 = parts
            .iter()
            .map(|(t, _)| self.text.measure(style, t).0)
            .sum();
        let mut x = (area.right() - PAD) as f32 - width;
        let y = self.baseline(area.y, area.h, style);
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
                if let Some(color) = arrow(x - at.x, y - at.y, &self.palette) {
                    c.fill(gfx2d::Rect::new(x, y, 1, 1), color);
                }
            }
        }
    }
}

/// The arrow's pixel at `(x, y)` from its tip: `text` inside a `desk` edge,
/// None outside it. The hardware cursor's image is the same arrow.
pub fn arrow(x: i32, y: i32, palette: &Palette) -> Option<u32> {
    let width = y.min(11);
    if !(0..18).contains(&y) || !(0..=width).contains(&x) {
        return None;
    }
    let edge = x == 0 || x == width || y == 17;
    Some(opaque(if edge { palette.desk } else { palette.text }))
}

/// The baseline for `style` centred in a band `h` px tall from `top`: the
/// capitals' height, about 0.72 of the size, in the middle.
pub(crate) fn baseline(top: i32, h: i32, size: f32) -> i32 {
    top + libm::roundf((h as f32 + 0.72 * size) / 2.0) as i32
}

impl Painter {
    /// [`baseline`] for the theme's `style`.
    pub(crate) fn baseline(&self, top: i32, h: i32, style: TypeStyle) -> i32 {
        baseline(top, h, self.text.style(style).size)
    }
}

pub(crate) fn fill(c: &mut Canvas<'_>, r: Rect, clip: Rect, rgb: u32) {
    if let Some(r) = r.intersect(&clip) {
        c.fill(g(r), opaque(rgb));
    }
}

fn outline(c: &mut Canvas<'_>, r: Rect, clip: Rect, rgb: u32) {
    frame(c, r, clip, rgb, trusted::OUTLINE);
}

/// `r`'s edges, `t` px thick, inside it.
fn frame(c: &mut Canvas<'_>, r: Rect, clip: Rect, rgb: u32, t: i32) {
    for edge in [
        Rect::new(r.x, r.y, r.w, t),
        Rect::new(r.x, r.bottom() - t, r.w, t),
        Rect::new(r.x, r.y, t, r.h),
        Rect::new(r.right() - t, r.y, t, r.h),
    ] {
        fill(c, edge, clip, rgb);
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
pub(crate) fn disc(c: &mut Canvas<'_>, clip: Rect, at: (i32, i32), r: f32, rgb: u32) {
    let reach = libm::ceilf(r) as i32 + 1;
    for y in at.1 - reach..=at.1 + reach {
        for x in at.0 - reach..=at.0 + reach {
            let (dx, dy) = ((x - at.0) as f32, (y - at.1) as f32);
            let d = libm::sqrtf(dx * dx + dy * dy);
            plot(c, clip, x, y, rgb, r + 0.5 - d);
        }
    }
}

/// How a title-bar control is drawn.
#[derive(Clone, Copy)]
struct Shown {
    focused: bool,
    hovered: bool,
    maximized: bool,
}

/// Our glyph for the app whose kernel label is `label` (`notes-window` is
/// Notes), or none for an app we have none for.
pub(crate) fn app_icon(label: &str) -> Option<Icon> {
    let app = label.strip_suffix("-window").unwrap_or(label);
    match app {
        "term" | "terminal" => Some(Icon::Terminal),
        "files" => Some(Icon::Files),
        "notes" => Some(Icon::Notes),
        "editor" => Some(Icon::Editor),
        "monitor" => Some(Icon::Monitor),
        "models" => Some(Icon::Models),
        "settings" => Some(Icon::Settings),
        _ => None,
    }
}

/// A hovered control's glyph, on `hover`.
pub(crate) fn hover_ink(palette: &Palette) -> u32 {
    palette.text
}

/// A monogram's letter, on its `hover` tile.
pub(crate) fn monogram_ink(palette: &Palette) -> u32 {
    palette.muted
}
