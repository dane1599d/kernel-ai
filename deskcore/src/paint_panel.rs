//! The panel's drawing (ADR 0005 §8): `kai>` in bold ember, the line or its
//! placeholder, the ember block caret and a 2 px ember rule while it listens,
//! and on the right what the shell reports: the model's dot when idle,
//! "Kai is getting ready" while the model warms up, idle or "4s" into a
//! request, "thinking 4s" with "Esc stops", the call being acted, the result line with
//! undo, or a refusal under an `error` rule. A line longer than the input
//! scrolls so the caret stays in view, a dim `<` marking text cut off on the
//! left ([`view`]).

use crate::geom::Rect;
use crate::paint::{PAD, Painter, Scene, disc, fill, g};
use crate::panel::{Panel, State};
use alloc::format;
use alloc::string::String;
use gfx2d::Canvas;
use look::TypeStyle;

const PROMPT: &str = "kai>";
const PLACEHOLDER: &str = "ask, or type Rhai";
/// What the panel says while the model warms up (founder test 2).
pub const GETTING_READY: &str = "Kai is getting ready";
/// The rule along the panel's top while it listens or refuses.
const RULE: i32 = 2;
/// Drawn in the input's first column while the line's start is scrolled off.
const CUT: &str = "<";

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

/// The panel's right half, where the shell's state is drawn: what a state
/// change damages.
pub fn state_area(panel: Rect) -> Rect {
    Rect::new(panel.x + panel.w / 2, panel.y, panel.w / 2, panel.h)
}

impl Painter {
    pub(crate) fn panel(&mut self, c: &mut Canvas<'_>, clip: Rect, scene: &Scene<'_>) {
        let area = scene.desk.panel();
        let Some(clip) = area.intersect(&clip) else {
            return;
        };
        let panel = scene.panel;
        fill(c, area, clip, self.palette.window);
        let (rule, thick) = match (panel.state(), panel.focused()) {
            (State::Refused { .. }, _) => (self.palette.error, RULE),
            (_, true) => (self.palette.ai, RULE),
            _ => (self.palette.line, 1),
        };
        fill(c, Rect::new(area.x, area.y, area.w, thick), clip, rule);
        let (advance, line_h, _) = self.text.cell(1.0);
        let y = self.baseline(area.y, area.h, TypeStyle::Mono);
        let x = (area.x + PAD) as f32;
        let x = x + self
            .text
            .draw_mono(c, g(clip), x, y, 1.0, true, self.palette.ai, PROMPT);
        let input_x = x + advance;
        let input = Rect::new(input_x as i32, area.y, area.w / 2 - input_x as i32, area.h);
        let input_clip = input.intersect(&clip).unwrap_or_default();
        let cols = libm::floorf(input.w.max(0) as f32 / advance) as usize;
        let skip = view(panel.text().chars().count(), panel.cursor(), cols);
        // The column the text starts in: after the marker when scrolled.
        let text_x = match skip {
            0 => input_x,
            _ => {
                self.text.draw_mono(
                    c,
                    g(input_clip),
                    input_x,
                    y,
                    1.0,
                    false,
                    self.palette.dim,
                    CUT,
                );
                input_x + advance
            }
        };
        if panel.text().is_empty() && !panel.focused() {
            self.draw_text(
                c,
                input_clip,
                input_x,
                y,
                TypeStyle::Mono,
                self.palette.dim,
                PLACEHOLDER,
            );
        } else {
            let shown: String = panel.text().chars().skip(skip).collect();
            self.text.draw_mono(
                c,
                g(input_clip),
                text_x,
                y,
                1.0,
                false,
                self.palette.text,
                &shown,
            );
        }
        if panel.focused() {
            let column = panel.cursor().saturating_sub(skip);
            self.caret(c, input_clip, panel, text_x, column, (advance, line_h));
        }
        self.state(c, clip, area, scene);
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

    /// The shell's state, right-aligned in the right half.
    fn state(&mut self, c: &mut Canvas<'_>, clip: Rect, area: Rect, scene: &Scene<'_>) {
        let right = state_area(area);
        let Some(clip) = right.intersect(&clip) else {
            return;
        };
        let style = TypeStyle::MonoTitle;
        let y = self.baseline(area.y, area.h, style);
        let end = (right.right() - PAD) as f32;
        let parts: alloc::vec::Vec<(String, u32)> = match scene.panel.state() {
            // The model is up but not warm: a request now waits for the warm-up.
            State::Idle if scene.panel.getting_ready() => {
                alloc::vec![(String::from(GETTING_READY), self.palette.dim)]
            }
            State::Thinking { since_ms } if scene.panel.getting_ready() => {
                let seconds = scene.now_ms.saturating_sub(*since_ms) / 1000;
                alloc::vec![
                    (format!("{GETTING_READY} {seconds}s"), self.palette.text),
                    (String::from("   Esc stops"), self.palette.dim),
                ]
            }
            State::Idle => {
                let dot = if scene.bar.model {
                    self.palette.ok
                } else {
                    self.palette.dim
                };
                disc(
                    c,
                    clip,
                    (right.right() - PAD - 3, area.y + area.h / 2),
                    3.0,
                    dot,
                );
                return;
            }
            State::Thinking { since_ms } => {
                let seconds = scene.now_ms.saturating_sub(*since_ms) / 1000;
                alloc::vec![
                    (format!("thinking {seconds}s"), self.palette.text),
                    (String::from("   Esc stops"), self.palette.dim),
                ]
            }
            State::Acting { call, target } => alloc::vec![
                (call.clone(), self.palette.text),
                (format!("  {target}"), self.palette.muted),
            ],
            State::Done { line } => alloc::vec![
                (line.clone(), self.palette.text),
                (String::from("   undo  show"), self.palette.accent),
            ],
            State::Refused { reason } => alloc::vec![(reason.clone(), self.palette.error)],
        };
        let width: f32 = parts
            .iter()
            .map(|(text, _)| self.text.measure(style, text).0)
            .sum();
        let mut x = (end - width).max(right.x as f32);
        if matches!(scene.panel.state(), State::Done { .. }) {
            // The check before a finished line.
            let at = (x as i32 - 12, area.y + area.h / 2);
            disc(c, clip, at, 3.0, self.palette.ok);
        }
        for (text, color) in &parts {
            x += self.draw_text(c, clip, x, y, style, *color, text);
        }
    }
}

/// Whether the shown state changes with the clock: "thinking 4s" counts.
pub fn ticks(panel: &Panel) -> bool {
    matches!(panel.state(), State::Thinking { .. })
}
