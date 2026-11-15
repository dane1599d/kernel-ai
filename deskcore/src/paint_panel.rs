//! The panel's input and status row, as the Kai pane draws them (ADR 0005
//! §8): `kai>` in bold ember, the line or its placeholder, and the ember
//! block caret with a 2 px ember rule while it listens; and in the status
//! row what the shell reports, code's alone: the model's dot and "Ready",
//! "Kai is getting ready" while the model warms, "thinking 4s · Esc stops",
//! the call being acted, the result line with undo and show, or a refusal
//! under an `error` bar. A line longer than the input scrolls so the caret
//! stays in view, a dim `<` marking text cut off on the left ([`view`]).

use crate::desk::Desk;
use crate::geom::{Point, Rect};
use crate::paint::{Painter, Scene, disc, fill, g};
use crate::panel::{Panel, State};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::Canvas;
use look::TypeStyle;

const PROMPT: &str = "kai>";
const PLACEHOLDER: &str = "Ask Kai, or type Rhai";
/// What the input and status say while the model warms up (founder test 2).
pub const GETTING_READY: &str = "Kai is getting ready";
/// The input's rule while it listens, and the status row's while refused.
const RULE: i32 = 2;
const ERROR_BAR: i32 = 3;
/// Drawn in the input's first column while the line's start is scrolled off.
const CUT: &str = "<";
/// The status row's and the input's text inset.
const INSET: i32 = 12;
/// Room for the done line's check, or the idle dot, before the text.
const MARK: i32 = 12;
/// The links a done line ends with, as drawn: what each sends when clicked.
/// They sit at the row's right end, so a long line is cut before them and
/// never pushes them out of the pane.
const LINKS: &str = "   undo  show";
/// Where the status row's text ends, from its right edge, with no Stop.
const END: i32 = 12;

/// The status row's text end, with no Stop item: where the links end.
pub fn status_end(row: Rect) -> i32 {
    row.right() - END
}

/// How many characters of a line `len` long to skip so the cursor (at most
/// one past the end) stays in an input `cols` columns wide. Nothing is
/// skipped while the cursor fits; once it does not, the first column is the
/// [`CUT`] marker and the cursor sits in the last column.
pub fn view(len: usize, cursor: usize, cols: usize) -> usize {
    let cursor = cursor.min(len);
    if cols < 2 || cursor < cols {
        return 0;
    }
    cursor + 2 - cols
}

/// The pane's status row on the screen while the pane is open: what a state
/// change damages. Empty while it is closed.
pub fn state_area(desk: &Desk) -> Rect {
    match desk.pane() {
        Some(_) => desk.layout().pane_parts().status,
        None => Rect::default(),
    }
}

/// What changed in the panel, for what it repaints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelChange {
    /// A key edited the line: the input alone.
    LineEdit,
    /// The bar (the model, its warmth): the status row's dot and the
    /// header's state.
    Bar,
    /// A new state, focus or anything else: the whole pane.
    Pane,
}

/// What `change` repaints on `desk`: nothing while the pane is closed.
pub fn panel_damage(desk: &Desk, change: PanelChange) -> alloc::vec::Vec<Rect> {
    match change {
        PanelChange::LineEdit => alloc::vec![input_area(desk)],
        PanelChange::Bar => alloc::vec![state_area(desk), crate::paint_pane::header_area(desk)],
        PanelChange::Pane => alloc::vec![desk.pane_area()],
    }
}

/// The pane's input on the screen while it is open; empty while closed.
pub fn input_area(desk: &Desk) -> Rect {
    match desk.pane() {
        Some(_) => desk.layout().pane_parts().input,
        None => Rect::default(),
    }
}

/// What a click on the pane's input or status row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneClick {
    /// The send item, with Kai idle: Enter.
    Send,
    /// The send item or the status row's Stop while Kai thinks: exactly Esc.
    Stop,
    /// A done line's `undo` or `show`, sent as though typed.
    Link(&'static str),
}

/// Whether the shown state changes with the clock: "thinking 4s" counts.
pub fn ticks(panel: &Panel) -> bool {
    matches!(panel.state(), State::Thinking { .. })
}

impl Painter {
    /// The input field's inside, `field` in the canvas's px, its text ending
    /// before `end`: the prompt, the line or its placeholder, the caret.
    pub(crate) fn input(&mut self, c: &mut Canvas<'_>, field: Rect, end: i32, scene: &Scene<'_>) {
        let panel = scene.panel;
        if panel.focused() {
            let rule = Rect::new(field.x + 1, field.y + 1, field.w - 2, RULE);
            fill(c, rule, field, self.palette.ai);
        }
        let (advance, line_h, _) = self.text.cell(1.0);
        let y = self.baseline(field.y, field.h, TypeStyle::Mono);
        let x = (field.x + INSET) as f32;
        let x = x + self
            .text
            .draw_mono(c, g(field), x, y, 1.0, true, self.palette.ai, PROMPT);
        let input_x = x + advance;
        let input = Rect::new(input_x as i32, field.y, end - input_x as i32, field.h);
        let clip = input.intersect(&field).unwrap_or_default();
        let cols = libm::floorf(input.w.max(0) as f32 / advance) as usize;
        let skip = view(panel.text().chars().count(), panel.cursor(), cols);
        // The column the text starts in: after the marker when scrolled.
        let text_x = match skip {
            0 => input_x,
            _ => {
                let dim = self.palette.dim;
                self.text
                    .draw_mono(c, g(clip), input_x, y, 1.0, false, dim, CUT);
                input_x + advance
            }
        };
        if panel.text().is_empty() && !panel.focused() {
            let hint = match panel.getting_ready() {
                true => GETTING_READY,
                false => PLACEHOLDER,
            };
            let dim = self.palette.dim;
            self.draw_text(c, clip, input_x, y, TypeStyle::Mono, dim, hint);
        } else {
            let shown: String = panel.text().chars().skip(skip).collect();
            let text = self.palette.text;
            self.text
                .draw_mono(c, g(clip), text_x, y, 1.0, false, text, &shown);
        }
        if panel.focused() {
            let column = panel.cursor().saturating_sub(skip);
            self.caret(c, clip, panel, text_x, column, (advance, line_h));
        }
    }

    /// The ember block at the cursor, `column` cells from `text_x`, the
    /// character under it drawn in `window`.
    fn caret(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        panel: &Panel,
        text_x: f32,
        column: usize,
        (advance, line_h): (f32, i32),
    ) {
        let x = text_x + advance * column as f32;
        let top = clip.y + (clip.h - line_h) / 2;
        let block = Rect::new(
            libm::roundf(x) as i32,
            top,
            libm::ceilf(advance) as i32,
            line_h,
        );
        fill(c, block, clip, self.palette.ai);
        if let Some(ch) = panel.text().chars().nth(panel.cursor()) {
            let y = self.baseline(clip.y, clip.h, TypeStyle::Mono);
            let under = String::from(ch);
            self.text
                .draw_mono(c, g(clip), x, y, 1.0, false, self.palette.window, &under);
        }
    }

    /// The shell's state in the status `row`, left-aligned, its text ending
    /// before `end`. Code's alone: it reads the panel's state, the model's
    /// warmth and the bar, never what anyone said.
    pub(crate) fn status(&mut self, c: &mut Canvas<'_>, row: Rect, end: i32, scene: &Scene<'_>) {
        let style = TypeStyle::MonoTitle;
        let y = self.baseline(row.y, row.h, style);
        let mid = row.y + row.h / 2;
        let x0 = row.x + INSET;
        let clip = Rect::new(row.x, row.y, end - row.x, row.h);
        let p = &self.palette;
        let (mark, parts): (Option<u32>, Vec<(String, u32)>) = match scene.panel.state() {
            // The model is up but not warm: a request now waits for it.
            State::Idle if scene.panel.getting_ready() => {
                (None, alloc::vec![(String::from(GETTING_READY), p.dim)])
            }
            State::Idle if scene.bar.model => {
                (Some(p.ok), alloc::vec![(String::from("Ready"), p.muted)])
            }
            State::Idle => (
                Some(p.dim),
                alloc::vec![(String::from("No model"), p.muted)],
            ),
            State::Thinking { since_ms } => {
                let seconds = scene.now_ms.saturating_sub(*since_ms) / 1000;
                let what = match scene.panel.getting_ready() {
                    true => GETTING_READY,
                    false => "thinking",
                };
                (
                    None,
                    alloc::vec![
                        (format!("{what} {seconds}s"), p.text),
                        (String::from(" · Esc stops"), p.dim),
                    ],
                )
            }
            State::Acting { call, target } => (
                None,
                alloc::vec![(call.clone(), p.text), (format!("  {target}"), p.muted)],
            ),
            // Its undo and show only with the log entry they act on, at the
            // row's right end.
            State::Done { line } => {
                if scene.panel.entry().is_some() {
                    let style = TypeStyle::MonoTitle;
                    let links_x = end as f32 - self.text.measure(style, LINKS).0;
                    let accent = p.accent;
                    self.draw_text(c, clip, links_x, y, style, accent, LINKS);
                    let cut = Rect::new(row.x, row.y, links_x as i32 - row.x, row.h);
                    let x = (x0 + MARK) as f32;
                    disc(c, clip, (x0 + 3, mid), 3.0, self.palette.ok);
                    let text = self.palette.text;
                    self.draw_text(c, cut, x, y, style, text, line);
                    return;
                }
                (Some(p.ok), alloc::vec![(line.clone(), p.text)])
            }
            State::Refused { reason } => {
                let bar = Rect::new(row.x, row.y, row.w, ERROR_BAR);
                fill(c, bar, row, p.error);
                (None, alloc::vec![(reason.clone(), p.error)])
            }
        };
        let mut x = x0 as f32;
        if let Some(dot) = mark {
            disc(c, clip, (x0 + 3, mid), 3.0, dot);
            x += MARK as f32;
        }
        for (text, color) in &parts {
            x += self.draw_text(c, clip, x, y, style, *color, text);
        }
    }

    /// What a click at `at` on the open pane does: the send item, the Stop
    /// item while Kai thinks, or a done line's link where it is drawn.
    pub fn pane_click(&self, desk: &Desk, panel: &Panel, at: Point) -> Option<PaneClick> {
        desk.pane()?;
        let parts = desk.layout().pane_parts();
        let thinking = matches!(panel.state(), State::Thinking { .. });
        if parts.send.contains(at) || (thinking && parts.stop.contains(at)) {
            return Some(match thinking {
                true => PaneClick::Stop,
                false => PaneClick::Send,
            });
        }
        self.panel_link(parts.status, panel, at)
            .map(PaneClick::Link)
    }

    /// Which of a done line's links, `undo` or `show`, the point `at` on the
    /// screen is on, as [`Painter::status`] draws them in the status `row`:
    /// what a click there sends, as though typed. `None` off them, or with
    /// no done line.
    pub fn panel_link(&self, row: Rect, shown: &Panel, at: Point) -> Option<&'static str> {
        let State::Done { .. } = shown.state() else {
            return None;
        };
        // No links are drawn without the entry they act on.
        shown.entry()?;
        if !row.contains(at) {
            return None;
        }
        let style = TypeStyle::MonoTitle;
        let width = |t: &str| self.text.measure(style, t).0;
        let links = status_end(row) as f32 - width(LINKS);
        let x = at.x as f32;
        let hit = |from: &str, word: &str| {
            let left = links + width(from);
            x >= left - 4.0 && x <= left + width(word) + 4.0
        };
        match (hit("   ", "undo"), hit("   undo  ", "show")) {
            (true, _) => Some("undo"),
            (_, true) => Some("show"),
            _ => None,
        }
    }
}
