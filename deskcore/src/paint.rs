//! Drawing the desk (ADR 0005 §5, §8): the desk, each window's chrome and
//! content, the focused window's shadow, the agent's outline, the taskbar and
//! tray, the Kai pane, Start, a tooltip, the ask card and the cursor, in the
//! theme's tokens and type (the dark theme until
//! [`Painter::set_theme`]). The card is
//! drawn last but the cursor, so nothing else covers it (ADR 0005 §7), and
//! the painter says what it drew over the windows ([`Painter::covered`]), so
//! no client's own px are put on any of that at a scale above 1.0 either.
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
/// The software cursor's box, for its damage.
pub const CURSOR: i32 = 18;

/// What the tray and the pane show of the system: the model, the memory in
/// use and the wall clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    /// A process named `llm` runs.
    pub model: bool,
    pub mem_used_mib: u64,
    /// Seconds since 1970 UTC, for the tray's clock; none without a wall
    /// clock, and then no clock is drawn.
    pub utc_s: Option<u64>,
    /// The clock's fixed offset from UTC in minutes (`time_zone`); 0 is UTC.
    pub zone_min: i16,
}

/// Everything a frame shows besides the windows' own pixels.
pub struct Scene<'a> {
    pub desk: &'a Desk,
    pub bar: &'a Bar,
    pub panel: &'a Panel,
    /// The panel's history, drawn in the Kai pane.
    pub chat: &'a crate::chat::Chat,
    /// Start, while it is open: over the windows, the taskbar and the pane,
    /// under the card.
    pub start: Option<&'a crate::start::Start>,
    /// The text of the ask card up now, as the kernel posted it.
    pub ask: Option<&'a Card>,
    /// A send's text, checked against its card's digest: drawn in the pane
    /// in place of the history while that card is up.
    pub preview: Option<&'a crate::sendview::SendPreview>,
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
    /// What the last [`Painter::paint`] (and [`Painter::cursor`]) drew over
    /// the windows ([`Painter::covered`]).
    pub(crate) covered: alloc::vec::Vec<Rect>,
    /// What the shadows drawn in the last [`Painter::paint`] left of the light
    /// at each px of the frame (255: none), for [`Painter::shade`].
    pub(crate) shade: alloc::vec::Vec<u8>,
    /// Whether [`Painter::paint`] keeps [`Painter::shade`]: on unless
    /// [`Painter::track_shade`] says no client's own px need it.
    tracks_shade: bool,
}

/// `0xFF` alpha on a token.
pub(crate) const fn opaque(rgb: u32) -> u32 {
    (0xFF << 24) | rgb
}

pub(crate) fn g(r: Rect) -> gfx2d::Rect {
    gfx2d::Rect::new(r.x, r.y, r.w, r.h)
}

/// The tray's part whose text changes while the desk is still: what the
/// screendump goldens mask (deskgeom's tray mask).
pub fn tray_area(desk: &Desk) -> Rect {
    desk.layout().tray.area
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
            covered: alloc::vec::Vec::new(),
            shade: alloc::vec::Vec::new(),
            tracks_shade: true,
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

    /// Every px the last [`Painter::paint`] inside its clip, and the
    /// [`Painter::cursor`] after it, drew over the windows: the agent's
    /// outline, the taskbar and tray, the Kai pane, Start, a tooltip, the ask
    /// card and the arrow, each cut to the clip, rounded corners left out only
    /// where the shape does not reach. Nothing in them is any window's, so
    /// `desktop` clears the overlay's mask there ([`crate::overlay`]): a
    /// client's own px never land on them, whatever colour they are painted.
    pub fn covered(&self) -> &[Rect] {
        &self.covered
    }

    /// The light the shadows of the last [`Painter::paint`] left at each px of
    /// the frame, a byte a px and as wide as the frame (255: untouched).
    /// A client drawn at the screen's scale has only a key colour in the frame
    /// under a shadow; [`crate::overlay`] darkens its own px by this, so the
    /// shadow of the focused window, the pane or Start falls on them too (H6-19).
    pub fn shade(&self) -> &[u8] {
        &self.shade
    }

    /// Whether the next [`Painter::paint`]s keep [`Painter::shade`]. Only a
    /// client drawn at the screen's scale reads it, so `desktop` turns it off
    /// when none is shown: the pass costs the full composite with Start and
    /// the pane open a third. The buffer is left as it was, which is right
    /// where it is read: a px a client owns was painted with it on.
    pub fn track_shade(&mut self, on: bool) {
        self.tracks_shade = on;
    }

    /// Draws the shadow of a window at `r` into `c`, and into
    /// [`Painter::shade`] when that is kept.
    pub(crate) fn cast(&mut self, c: &mut Canvas<'_>, r: Rect, clip: Rect) {
        for shadow in [&self.far, &self.near] {
            match self.tracks_shade {
                true => shadow.draw_shaded(c, &mut self.shade, g(r), g(clip)),
                false => shadow.draw(c, g(r), g(clip)),
            }
        }
    }

    /// Notes that `r`, its corners rounded to `radius` as
    /// [`gfx2d::Image::round_corners`] rounds them, was drawn over the
    /// windows inside `clip`: every px of it the shape reaches at all.
    pub(crate) fn cover(&mut self, r: Rect, radius: i32, clip: Rect) {
        let radius = radius.clamp(0, r.w.min(r.h) / 2);
        let covered = &mut self.covered;
        let mut add = |band: Rect| {
            if let Some(band) = band.intersect(&clip) {
                covered.push(band);
            }
        };
        for cy in 0..radius {
            let inset = (0..radius)
                .find(|&cx| gfx2d::canvas::corner_alpha(radius, cx, cy) > 0)
                .unwrap_or(radius);
            let w = r.w - 2 * inset;
            add(Rect::new(r.x + inset, r.y + cy, w, 1));
            add(Rect::new(r.x + inset, r.bottom() - 1 - cy, w, 1));
        }
        add(Rect::new(r.x, r.y + radius, r.w, r.h - 2 * radius));
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
        let desk = scene.desk;
        self.covered.clear();
        if self.tracks_shade {
            let (width, height) = (c.width as usize, c.height as usize);
            if self.shade.len() != width * height {
                self.shade = alloc::vec![255; width * height];
            }
            let cut = g(clip).intersect(&c.bounds());
            for y in cut.y..cut.bottom() {
                let from = y as usize * width + cut.x as usize;
                if let Some(row) = self.shade.get_mut(from..from + cut.w as usize) {
                    row.fill(255);
                }
            }
        }
        c.fill(g(clip), opaque(self.palette.desk));
        for window in desk.windows().iter().filter(|w| !w.minimized) {
            if reach(window.frame).intersect(&clip).is_none() {
                continue;
            }
            let focused = desk.focus() == Focus::Window(window.id);
            if focused && desk.dragged() != Some(window.id) {
                self.cast(c, window.frame, clip);
            }
            let hovered = desk
                .hovered()
                .filter(|(id, _)| *id == window.id)
                .map(|(_, part)| part);
            let layer = self.layer(window, focused, hovered, content);
            let at = (window.frame.x, window.frame.y);
            let radius = chrome::radius(window.maximized());
            c.draw_window(&layer, at, g(clip), radius, true);
        }
        // The agent's outline over every window, its own and any stacked
        // above it, so no app covers where the agent acts; then the taskbar
        // and tray, then the Kai pane (ADR 0005 §7's paint order).
        let agent = desk.agent().and_then(|id| desk.window(id));
        if let Some(window) = agent.filter(|w| !w.minimized) {
            let ring = trusted::outline(window.frame);
            for edge in edges(ring, trusted::OUTLINE) {
                fill(c, edge, clip, self.palette.ai);
                self.cover(edge, 0, clip);
            }
        }
        self.taskbar(c, clip, scene);
        self.pane(c, clip, scene);
        // Start, then a tooltip, over both and under the card.
        self.start(c, clip, scene);
        self.tooltip(c, clip, scene);
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
                    self.cover(card, 0, clip);
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

    /// The pointer, drawn by `desktop` when the screen has no hardware cursor;
    /// its px join [`Painter::covered`], the box round the arrow does not.
    pub fn cursor(&mut self, c: &mut Canvas<'_>, clip: Rect, at: Point) {
        let r = cursor_box(at).intersect(&clip).unwrap_or_default();
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                if let Some(color) = arrow(x - at.x, y - at.y, &self.palette) {
                    c.fill(gfx2d::Rect::new(x, y, 1, 1), color);
                }
            }
        }
        // The arrow's row `y` is its px 0..=min(y, 11) ([`arrow`]).
        for y in 0..CURSOR {
            let row = Rect::new(at.x, at.y + y, y.min(11) + 1, 1);
            self.cover(row, 0, clip);
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

/// `r`'s edges, `t` px thick, inside it.
pub(crate) fn frame(c: &mut Canvas<'_>, r: Rect, clip: Rect, rgb: u32, t: i32) {
    for edge in edges(r, t) {
        fill(c, edge, clip, rgb);
    }
}

/// `r`'s four edges, `t` px thick, inside it.
fn edges(r: Rect, t: i32) -> [Rect; 4] {
    [
        Rect::new(r.x, r.y, r.w, t),
        Rect::new(r.x, r.bottom() - t, r.w, t),
        Rect::new(r.x, r.y, t, r.h),
        Rect::new(r.right() - t, r.y, t, r.h),
    ]
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
