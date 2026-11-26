//! Start's drawing (ADR 0005 §8): `raised`, radius 8, a 1 px `line_strong`
//! frame and the flyout shadow, over the windows, the taskbar and the pane,
//! under the card. The box at the top ("Ask Kai or search apps", or "Kai is
//! getting ready · search apps" while the model warms), focused with a 2 px
//! `ai` rule and the ember caret; then the view: Home (the pinned grid,
//! your own recent lines, the footer), Search ("Ask Kai: <text>" and the
//! matching apps) or All apps. A selected row takes the `hover` fill and a
//! 3 px `accent` edge; ember is Kai's alone. Rows are the catalog's data.

use crate::catalog::{App, Source};
use crate::chat::Who;
use crate::geom::{Point, Rect};
use crate::paint::{Painter, Scene, app_icon, fill, g};
use crate::paint_panel::view;
use crate::start::{Pick, Row, Start, View};
use alloc::string::String;
use alloc::vec::Vec;
use deskgeom::start::{self as geo, StartParts};
use gfx2d::icons::Icon;
use gfx2d::{Canvas, Image};
use look::TypeStyle;

const RADIUS: i32 = look::metric::RADIUS_MD as i32;
const FIELD_RADIUS: i32 = 20;
const ROW_RADIUS: i32 = 4;
const EDGE: i32 = 3;
const PLACEHOLDER: &str = "Ask Kai or search apps";
const WARMING: &str = "Kai is getting ready \u{b7} search apps";
const CUT: &str = "<";

/// Start's pinned apps ([`Start::pinned`]).
pub fn pinned(start: &Start) -> Vec<usize> {
    start.pinned()
}

/// Your own recent lines, newest first, at most 5: never Kai's words.
pub fn recent(chat: &crate::chat::Chat) -> Vec<String> {
    chat.entries()
        .rev()
        .filter(|e| e.who == Who::You)
        .map(|e| e.text.clone())
        .take(geo::RECENT_ROWS as usize)
        .collect()
}

/// The first All apps row shown, so the selected one is in view.
fn first_shown(start: &Start) -> usize {
    (start.sheet().selected_index() + 1).saturating_sub(geo::ALL_ROWS as usize)
}

/// What a click at `at` inside Start does: a tile, a recent line, "All
/// apps ›", Settings, or a row. True when Start changed without a pick.
pub fn click(
    start: &mut Start,
    area: Rect,
    at: Point,
    chat: &crate::chat::Chat,
) -> (bool, Option<Pick>) {
    let parts = geo::parts(area);
    match start.view() {
        View::Home => {
            if parts.all_apps.contains(at) {
                start.show_all();
                return (true, None);
            }
            if parts.settings.contains(at) {
                return (false, Some(Pick::Open(String::from("settings"))));
            }
            for (n, i) in pinned(start).into_iter().enumerate() {
                if geo::cell(area, n as i32).contains(at) {
                    return (false, Some(crate::start::open(&start.apps()[i])));
                }
            }
            for (n, line) in recent(chat).iter().enumerate() {
                if geo::recent_row(area, n as i32).contains(at) {
                    start.set_text(line);
                    return (true, None);
                }
            }
            (false, None)
        }
        View::Search => {
            let rows = start.rows().len() as i32;
            let hit = (0..rows).find(|&n| geo::search_row(area, n).contains(at));
            (false, hit.and_then(|n| start.pick_row(n as usize)))
        }
        View::AllApps => {
            let first = first_shown(start);
            let hit = (0..geo::ALL_ROWS).find(|&n| geo::all_row(area, n).contains(at));
            let app = hit.and_then(|n| start.apps().get(first + n as usize));
            (false, app.map(crate::start::open))
        }
    }
}

impl Painter {
    /// Start, if it is open, inside `clip`.
    pub(crate) fn start(&mut self, c: &mut Canvas<'_>, clip: Rect, scene: &Scene<'_>) {
        let (Some(at), Some(start)) = (scene.desk.start(), scene.start) else {
            return;
        };
        if crate::paint::reach(at).intersect(&clip).is_none() {
            return;
        }
        self.far.draw(c, g(at), g(clip));
        self.near.draw(c, g(at), g(clip));
        // The pointer over Start, in its own px: what takes the hover fill.
        let p = scene.desk.pointer();
        let hover = at.contains(p).then(|| Point {
            x: p.x - at.x,
            y: p.y - at.y,
        });
        let layer = self.start_layer(at, start, scene, hover);
        c.draw_window(&layer, (at.x, at.y), g(clip), RADIUS, true);
        self.cover(at, RADIUS, clip);
    }

    fn start_layer(
        &mut self,
        at: Rect,
        start: &Start,
        scene: &Scene<'_>,
        hover: Option<Point>,
    ) -> Image {
        let local = Rect::new(0, 0, at.w, at.h);
        let parts = geo::parts(local);
        let mut layer = Image::new(local.w, local.h);
        let mut c = layer.canvas();
        fill(&mut c, local, local, self.palette.line_strong);
        fill(&mut c, local.grow(-1), local, self.palette.raised);
        self.start_field(&mut c, parts.field, start, scene);
        match start.view() {
            View::Home => self.start_home(&mut c, &parts, start, scene, hover),
            View::Search => self.start_search(&mut c, local, start, hover),
            View::AllApps => self.start_all(&mut c, local, start, hover),
        }
        layer.round_corners(RADIUS);
        layer
    }

    /// The box: a pill in `window`, the search glyph, the line or its
    /// placeholder, and while focused the `ai` rule and the ember caret.
    fn start_field(&mut self, c: &mut Canvas<'_>, field: Rect, start: &Start, scene: &Scene<'_>) {
        self.rounded(c, field, field, self.palette.line_strong, FIELD_RADIUS);
        self.rounded(
            c,
            field.grow(-1),
            field,
            self.palette.window,
            FIELD_RADIUS - 1,
        );
        let muted = self.palette.muted;
        let glyph = (field.x + 14, field.y + (field.h - 16) / 2);
        self.icons
            .draw(c, (Icon::Search, 16), glyph, muted, g(field));
        let rule = Rect::new(
            field.x + FIELD_RADIUS,
            field.bottom() - 3,
            field.w - 2 * FIELD_RADIUS,
            2,
        );
        fill(c, rule, field, self.palette.ai);
        let text_x = field.x + 40;
        let input = Rect::new(text_x, field.y, field.right() - 16 - text_x, field.h);
        if start.text().is_empty() {
            let hint = match scene.panel.getting_ready() {
                true => WARMING,
                false => PLACEHOLDER,
            };
            let y = self.baseline(field.y, field.h, TypeStyle::Body);
            let dim = self.palette.dim;
            let after_caret = text_x as f32 + self.text.cell(1.0).0 + 4.0;
            self.draw_text(c, input, after_caret, y, TypeStyle::Body, dim, hint);
        }
        let (advance, line_h, _) = self.text.cell(1.0);
        let cols = libm::floorf(input.w.max(0) as f32 / advance) as usize;
        let skip = view(start.text().chars().count(), start.cursor(), cols);
        let y = self.baseline(field.y, field.h, TypeStyle::Mono);
        let mut x = text_x as f32;
        if skip > 0 {
            let dim = self.palette.dim;
            self.text.draw_mono(c, g(input), x, y, 1.0, false, dim, CUT);
            x += advance;
        }
        let shown: String = start.text().chars().skip(skip).collect();
        let text = self.palette.text;
        self.text
            .draw_mono(c, g(input), x, y, 1.0, false, text, &shown);
        let column = start.cursor().saturating_sub(skip);
        let caret_x = libm::roundf(x + advance * column as f32) as i32;
        let block = Rect::new(
            caret_x,
            field.y + (field.h - line_h) / 2,
            libm::ceilf(advance) as i32,
            line_h,
        );
        fill(c, block, input, self.palette.ai);
    }

    /// Home: "Pinned" and its link, the grid, "Recent" and your lines, and
    /// the footer with the model's state and Settings.
    fn start_home(
        &mut self,
        c: &mut Canvas<'_>,
        parts: &StartParts,
        start: &Start,
        scene: &Scene<'_>,
        hover: Option<Point>,
    ) {
        let area = parts.area;
        let text = self.palette.text;
        let y = self.baseline(parts.pinned.y, parts.pinned.h, TypeStyle::Label);
        self.draw_text(
            c,
            parts.pinned,
            parts.pinned.x as f32,
            y,
            TypeStyle::Label,
            text,
            "Pinned",
        );
        let accent = self.palette.accent;
        let link = parts.all_apps;
        self.draw_text(
            c,
            link,
            link.x as f32,
            y,
            TypeStyle::Label,
            accent,
            "All apps \u{203a}",
        );
        for (n, i) in pinned(start).into_iter().enumerate() {
            let cell = geo::cell(area, n as i32);
            let lit = start.home_selected() == Some(n) || hover.is_some_and(|p| cell.contains(p));
            if lit {
                self.start_fill(c, cell.grow(-2), area);
            }
            self.start_tile(c, cell, &start.apps()[i]);
        }
        let y = self.baseline(parts.recent.y, parts.recent.h, TypeStyle::Label);
        self.draw_text(
            c,
            parts.recent,
            parts.recent.x as f32,
            y,
            TypeStyle::Label,
            text,
            "Recent",
        );
        let muted = self.palette.muted;
        for (n, line) in recent(scene.chat).iter().enumerate() {
            let row = geo::recent_row(area, n as i32);
            if hover.is_some_and(|p| row.contains(p)) {
                self.start_fill(c, row, area);
            }
            let y = self.baseline(row.y, row.h, TypeStyle::Mono);
            self.draw_text(c, row, row.x as f32, y, TypeStyle::Mono, muted, line);
        }
        let footer = parts.footer;
        let inside = Rect::new(footer.x + 1, footer.y, footer.w - 2, footer.h - 1);
        fill(c, inside, area, self.palette.shell);
        fill(
            c,
            Rect::new(footer.x, footer.y, footer.w, 1),
            area,
            self.palette.line,
        );
        let mark = (footer.x + geo::PAD, footer.y + (footer.h - 20) / 2);
        self.icons
            .draw(c, (Icon::Kai, 20), mark, self.palette.ai, g(footer));
        let state = match (scene.bar.model, scene.panel.getting_ready()) {
            (false, _) => "No model",
            (true, true) => "Kai is getting ready",
            (true, false) => "Kai is ready",
        };
        let y = self.baseline(footer.y, footer.h, TypeStyle::Small);
        let x = (footer.x + geo::PAD + 28) as f32;
        self.draw_text(c, footer, x, y, TypeStyle::Small, muted, state);
        let s = parts.settings;
        let at = (s.x + (s.w - 24) / 2, s.y + (s.h - 24) / 2);
        self.icons.draw(c, (Icon::Settings, 24), at, muted, g(s));
    }

    /// A pinned tile: a 32 px icon (or monogram) on a 40x40 tile at the
    /// cell's top, the name in Small under it, centred and cut to 88.
    fn start_tile(&mut self, c: &mut Canvas<'_>, cell: Rect, app: &App) {
        let tile = Rect::new(cell.x + (cell.w - 40) / 2, cell.y + 4, 40, 40);
        match app_icon(&app.name) {
            Some(icon) => {
                let at = (tile.x + 4, tile.y + 4);
                self.icons
                    .draw(c, (icon, 32), at, self.palette.text, g(tile));
            }
            None => {
                let side = crate::monogram::TILE;
                let at = (tile.x + (40 - side) / 2, tile.y + (40 - side) / 2);
                self.monogram(c, tile, at, &app.name);
            }
        }
        let name_box = Rect::new(cell.x + 4, cell.y + 56, 88, 20);
        let w = self.text.measure(TypeStyle::Small, &app.name).0;
        let x = name_box.x as f32 + ((88.0 - w) / 2.0).max(0.0);
        let y = self.baseline(name_box.y, name_box.h, TypeStyle::Small);
        let text = self.palette.text;
        self.draw_text(c, name_box, x, y, TypeStyle::Small, text, &app.name);
    }

    /// Search: Ask Kai first, then the matching apps, the selected one on
    /// the `hover` fill with its `accent` edge.
    fn start_search(
        &mut self,
        c: &mut Canvas<'_>,
        area: Rect,
        start: &Start,
        hover: Option<Point>,
    ) {
        for (n, row) in start.rows().iter().enumerate() {
            let r = geo::search_row(area, n as i32);
            let selected = n == start.selected();
            let hovered = hover.is_some_and(|p| r.contains(p));
            self.start_row_ground(c, r, area, selected, hovered);
            match row {
                Row::Ask => {
                    let at = (r.x + 12, r.y + (r.h - 24) / 2);
                    self.icons
                        .draw(c, (Icon::Kai, 24), at, self.palette.ai_mark, g(r));
                    let y = self.baseline(r.y, r.h, TypeStyle::Mono);
                    let text = self.palette.text;
                    let x = (r.x + 48) as f32;
                    let x = x + self.draw_text(c, r, x, y, TypeStyle::Label, text, "Ask Kai: ");
                    let muted = self.palette.muted;
                    self.draw_text(c, r, x, y, TypeStyle::Mono, muted, start.text().trim());
                }
                Row::App(i) => self.start_app_row(c, r, &start.apps()[*i]),
            }
        }
    }

    /// All apps: its header, then the catalog's rows in its order.
    fn start_all(&mut self, c: &mut Canvas<'_>, area: Rect, start: &Start, hover: Option<Point>) {
        let head = Rect::new(area.x + geo::PAD, area.y + 80, area.w - 2 * geo::PAD, 30);
        let y = self.baseline(head.y, head.h, TypeStyle::Label);
        let text = self.palette.text;
        self.draw_text(
            c,
            head,
            head.x as f32,
            y,
            TypeStyle::Label,
            text,
            "All apps",
        );
        let first = first_shown(start);
        let apps = start.apps().iter().enumerate().skip(first);
        for (n, (i, app)) in apps.take(geo::ALL_ROWS as usize).enumerate() {
            let r = geo::all_row(area, n as i32);
            let selected = i == start.sheet().selected_index();
            let hovered = hover.is_some_and(|p| r.contains(p));
            self.start_row_ground(c, r, area, selected, hovered);
            self.start_app_row(c, r, app);
        }
    }

    /// A row's ground: selected, the hover cue and a 3 px `accent` edge;
    /// under the pointer, the hover cue alone.
    fn start_row_ground(
        &mut self,
        c: &mut Canvas<'_>,
        r: Rect,
        clip: Rect,
        selected: bool,
        hovered: bool,
    ) {
        if selected || hovered {
            self.start_fill(c, r, clip);
        }
        if selected {
            let edge = Rect::new(r.x, r.y + 4, EDGE, r.h - 8);
            fill(c, edge, clip, self.palette.accent);
        }
    }

    /// The hover cue on Start's `raised` ground: the `hover` fill, radius 4,
    /// or where `hover` is that ground (high contrast) a 1 px `focus` frame.
    fn start_fill(&mut self, c: &mut Canvas<'_>, r: Rect, clip: Rect) {
        match self.palette.hover == self.palette.raised {
            true => crate::paint::frame(c, r, clip, self.palette.focus, 1),
            false => self.rounded(c, r, clip, self.palette.hover, ROW_RADIUS),
        }
    }

    /// An app row: its 24 px icon, its name, its summary (data) in `muted`,
    /// and Installed or Install at the right.
    fn start_app_row(&mut self, c: &mut Canvas<'_>, r: Rect, app: &App) {
        let at = (r.x + 12, r.y + (r.h - 24) / 2);
        match app_icon(&app.name) {
            Some(icon) => self.icons.draw(c, (icon, 24), at, self.palette.text, g(r)),
            None => self.monogram(c, r, (at.0 + 4, at.1 + 4), &app.name),
        }
        let x = (r.x + 48) as f32;
        let text = self.palette.text;
        let muted = self.palette.muted;
        let name_y = self.baseline(r.y + 4, 22, TypeStyle::Label);
        self.draw_text(c, r, x, name_y, TypeStyle::Label, text, &app.name);
        let tag = match app.source {
            Source::Installed | Source::BuiltIn => "Installed",
            _ => "Install",
        };
        let w = self.text.measure(TypeStyle::Small, tag).0;
        let tag_x = r.right() as f32 - 12.0 - w;
        self.draw_text(c, r, tag_x, name_y, TypeStyle::Small, muted, tag);
        let text_end = Rect::new(r.x, r.y, (tag_x as i32 - 8 - r.x).max(0), r.h);
        let sum_y = self.baseline(r.y + 26, 20, TypeStyle::Small);
        self.draw_text(c, text_end, x, sum_y, TypeStyle::Small, muted, &app.summary);
    }
}
