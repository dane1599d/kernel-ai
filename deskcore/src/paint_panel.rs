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
const GETTING_READY: &str = "Kai is getting ready";
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
/// A held send's link while it waits: undo, by the send's log entry.
const SEND_LINK: &str = "   undo";
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
    /// A held send's `undo`: `undo <entry>`, that send's and no other.
    UndoSend(u64),
}

/// Whether the shown state changes with the clock: "thinking 4s" counts,
/// and so does a held send's countdown.
pub fn ticks(panel: &Panel) -> bool {
    matches!(panel.state(), State::Thinking { .. })
        || panel.shown_send().is_some_and(|send| !send.sending)
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
        // A held send shows whenever no line is being worked on.
        if let Some(send) = scene.panel.shown_send() {
            let more = scene.panel.sends().len().saturating_sub(1);
            let text_end = match send.sending {
                true => end,
                false => {
                    let links_x = end as f32 - self.text.measure(style, SEND_LINK).0;
                    let accent = self.palette.accent;
                    self.draw_text(c, clip, links_x, y, style, accent, SEND_LINK);
                    links_x as i32
                }
            };
            let cut = Rect::new(row.x, row.y, text_end - row.x, row.h);
            let width = (text_end - x0 - MARK) as f32;
            let text = self.send_line(send, more, scene.now_ms, width);
            disc(c, clip, (x0 + 3, mid), 3.0, self.palette.ai);
            let color = self.palette.text;
            self.draw_text(c, cut, (x0 + MARK) as f32, y, style, color, &text);
            return;
        }
        // Held sends behind a newer line's result: said after it.
        let note = scene
            .panel
            .waiting_note()
            .map(|note| format!(" \u{b7} {note}"));
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
                    let line = format!("{line}{}", note.as_deref().unwrap_or(""));
                    self.draw_text(c, cut, x, y, style, text, &line);
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
        let mut parts = parts;
        if let Some(note) = note {
            parts.push((note, self.palette.dim));
        }
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
        if let Some(entry) = self.send_link(parts.status, panel, at) {
            return Some(PaneClick::UndoSend(entry));
        }
        self.panel_link(parts.status, panel, at)
            .map(PaneClick::Link)
    }

    /// The held send whose `undo` the point `at` is on, as
    /// [`Painter::status`] draws it at the row's right end; `None` off it,
    /// or once the send is going.
    pub fn send_link(&self, row: Rect, shown: &Panel, at: Point) -> Option<u64> {
        let send = shown.shown_send().filter(|send| !send.sending)?;
        if !row.contains(at) {
            return None;
        }
        let style = TypeStyle::MonoTitle;
        let width = |t: &str| self.text.measure(style, t).0;
        let left = status_end(row) as f32 - width(SEND_LINK) + width("   ");
        let x = at.x as f32;
        (x >= left - 4.0 && x <= left + width("undo") + 4.0).then_some(send.entry)
    }

    /// Which of a done line's links, `undo` or `show`, the point `at` on the
    /// screen is on, as [`Painter::status`] draws them in the status `row`:
    /// what a click there sends, as though typed. `None` off them, or with
    /// no done line.
    pub fn panel_link(&self, row: Rect, shown: &Panel, at: Point) -> Option<&'static str> {
        let State::Done { .. } = shown.state() else {
            return None;
        };
        // A held send shows in its place, with its own undo.
        if shown.shown_send().is_some() {
            return None;
        }
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

/// A held send as the status row says it, in code's words, around its
/// host: the countdown first ("0:04 · Sending to "), then how many more wait
/// (" · 1 more"); once it goes, "Sending to " and "...". With `short`, the
/// head is the countdown alone ("0:04 · "), for a narrow pane.
fn send_parts(
    send: &crate::sends::HeldSend,
    more: usize,
    now_ms: u64,
    short: bool,
) -> (String, String) {
    let more = match more {
        0 => String::new(),
        n => format!(" \u{b7} {n} more"),
    };
    let sending = if short { "" } else { "Sending to " };
    match send.sending {
        true => (String::from(sending), format!("...{more}")),
        false => (
            format!(
                "{} \u{b7} {sending}",
                crate::sends::left(send.due_ms, now_ms)
            ),
            more,
        ),
    }
}

/// A held send's whole line: "0:04 · Sending to hooks.example · 1 more".
pub fn send_text(send: &crate::sends::HeldSend, more: usize, now_ms: u64) -> String {
    let (head, tail) = send_parts(send, more, now_ms, false);
    format!("{head}{}{tail}", send.host)
}

/// How many characters of `host` may be cut from its left and leave its
/// last two labels, the site's own name: 25 for
/// `a-rather-long-host-name.hooks.example.org`.
fn cuttable(host: &str) -> usize {
    let mut dots = host.char_indices().rev().filter(|(_, c)| *c == '.');
    match dots.nth(1) {
        Some((at, _)) => host[..=at].chars().count(),
        None => 0,
    }
}

impl Painter {
    /// [`send_text`] in at most `width` px. The host is cut from its left
    /// ("...example.org") as far as its last two labels; past that, the head
    /// drops "Sending to"; only then is the site's name cut too. The
    /// countdown and the count always show.
    pub fn send_line(
        &self,
        send: &crate::sends::HeldSend,
        more: usize,
        now_ms: u64,
        width: f32,
    ) -> String {
        let fits = |text: &str| self.text.measure(TypeStyle::MonoTitle, text).0 <= width;
        let host: Vec<char> = send.host.chars().collect();
        let cut = |head: &str, tail: &str, skip: usize| match skip {
            0 => format!("{head}{}{tail}", send.host),
            _ => format!("{head}...{}{tail}", host[skip..].iter().collect::<String>()),
        };
        let keep = cuttable(&send.host);
        let (head, tail) = send_parts(send, more, now_ms, false);
        let (short, short_tail) = send_parts(send, more, now_ms, true);
        (0..=keep)
            .map(|skip| cut(&head, &tail, skip))
            .chain((0..host.len()).map(|skip| cut(&short, &short_tail, skip)))
            .find(|text| fits(text))
            .unwrap_or_else(|| format!("{short}...{short_tail}"))
    }
}

#[cfg(test)]
mod send_tests {
    #[test]
    fn the_sites_own_name_is_its_last_two_labels() {
        let host = "a-rather-long-host-name.hooks.example.org";
        assert_eq!(&host[super::cuttable(host)..], "example.org");
        assert_eq!(super::cuttable("example.org"), 0);
        assert_eq!(super::cuttable("localhost"), 0);
    }
}
