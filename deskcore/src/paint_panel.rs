//! The panel's drawing (ADR 0005 §8): `kai>` in bold ember, the line or its
//! placeholder, the ember block caret and a 2 px ember rule while it listens,
//! and on the right what the shell reports: the model's dot when idle,
//! "thinking 4s" with "Esc stops", the call being acted, the result line with
//! undo, or a refusal under an `error` rule.

use crate::geom::Rect;
use crate::paint::{PAD, Painter, Scene, baseline, disc, fill, g};
use crate::panel::{Panel, State};
use alloc::format;
use alloc::string::String;
use gfx2d::Canvas;
use look::{DARK, TypeStyle};

const PROMPT: &str = "kai>";
const PLACEHOLDER: &str = "ask, or type Rhai";
/// The rule along the panel's top while it listens or refuses.
const RULE: i32 = 2;

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
        fill(c, area, clip, DARK.window);
        let (rule, thick) = match (panel.state(), panel.focused()) {
            (State::Refused { .. }, _) => (DARK.error, RULE),
            (_, true) => (DARK.ai, RULE),
            _ => (DARK.line, 1),
        };
        fill(c, Rect::new(area.x, area.y, area.w, thick), clip, rule);
        let (advance, line_h, _) = self.text.cell(1.0);
        let y = baseline(area.y, area.h, TypeStyle::Mono);
        let x = (area.x + PAD) as f32;
        let x = x + self
            .text
            .draw_mono(c, g(clip), x, y, 1.0, true, DARK.ai, PROMPT);
        let input_x = x + advance;
        let input = Rect::new(input_x as i32, area.y, area.w / 2 - input_x as i32, area.h);
        let input_clip = input.intersect(&clip).unwrap_or_default();
        if panel.text().is_empty() && !panel.focused() {
            self.draw_text(
                c,
                input_clip,
                input_x,
                y,
                TypeStyle::Mono,
                DARK.dim,
                PLACEHOLDER,
            );
        } else {
            self.text.draw_mono(
                c,
                g(input_clip),
                input_x,
                y,
                1.0,
                false,
                DARK.text,
                panel.text(),
            );
        }
        if panel.focused() {
            self.caret(c, input_clip, panel, input_x, advance, line_h);
        }
        self.state(c, clip, area, scene);
    }

    /// The ember block at the cursor, the character under it drawn in `window`.
    fn caret(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        panel: &Panel,
        input_x: f32,
        advance: f32,
        line_h: i32,
    ) {
        let x = input_x + advance * panel.cursor() as f32;
        let top = clip.y + (clip.h - line_h) / 2;
        let block = Rect::new(
            libm::roundf(x) as i32,
            top,
            libm::ceilf(advance) as i32,
            line_h,
        );
        fill(c, block, clip, DARK.ai);
        if let Some(ch) = panel.text().chars().nth(panel.cursor()) {
            let y = baseline(clip.y, clip.h, TypeStyle::Mono);
            let under = String::from(ch);
            self.text
                .draw_mono(c, g(clip), x, y, 1.0, false, DARK.window, &under);
        }
    }

    /// The shell's state, right-aligned in the right half.
    fn state(&mut self, c: &mut Canvas<'_>, clip: Rect, area: Rect, scene: &Scene<'_>) {
        let right = state_area(area);
        let Some(clip) = right.intersect(&clip) else {
            return;
        };
        let style = TypeStyle::MonoTitle;
        let y = baseline(area.y, area.h, style);
        let end = (right.right() - PAD) as f32;
        let parts: alloc::vec::Vec<(String, u32)> = match scene.panel.state() {
            State::Idle => {
                let dot = if scene.bar.model { DARK.ok } else { DARK.dim };
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
                    (format!("thinking {seconds}s"), DARK.text),
                    (String::from("   Esc stops"), DARK.dim),
                ]
            }
            State::Acting { call, target } => alloc::vec![
                (call.clone(), DARK.text),
                (format!("  {target}"), DARK.muted),
            ],
            State::Done { line } => alloc::vec![
                (line.clone(), DARK.text),
                (String::from("   undo  show"), DARK.accent),
            ],
            State::Refused { reason } => alloc::vec![(reason.clone(), DARK.error)],
        };
        let width: f32 = parts
            .iter()
            .map(|(text, _)| self.text.measure(style, text).0)
            .sum();
        let mut x = (end - width).max(right.x as f32);
        if matches!(scene.panel.state(), State::Done { .. }) {
            // The check before a finished line.
            let at = (x as i32 - 12, area.y + area.h / 2);
            disc(c, clip, at, 3.0, DARK.ok);
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
