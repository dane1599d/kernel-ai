//! The desktop the scenes draw: wallpaper, the 28 px top bar, windows with
//! server-side chrome (32 px title bar, 1 px frame, 6 px radius, a shadow on the
//! focused one), the 36 px `kai>` panel in each state and the sheet above it
//! (ADR 0005 §8). Every size is logical px times `scale`. Compositing is damage
//! tracked: [`Desk::compose`] redraws one rectangle, bottom layer first.

use crate::frame::{Frame, Rect, Shadow};
use crate::look::Look;
use crate::text::{Family, Style, Text};
use alloc::string::String;
use alloc::vec::Vec;

pub struct Window {
    pub rect: Rect,
    /// Whether its body is opaque, so only the corner bands blend.
    pub opaque: bool,
    /// Premultiplied ARGB: the chrome, the content and the rounded corners.
    pub layer: Frame,
    pub title: &'static str,
    pub focused: bool,
}

/// What the panel shows (ADR 0005 §8's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelState {
    Idle,
    Listening,
    Thinking,
    Acting,
    Done,
    Asking,
    Refused,
}

impl PanelState {
    pub const ALL: [PanelState; 7] = [
        PanelState::Idle,
        PanelState::Listening,
        PanelState::Thinking,
        PanelState::Acting,
        PanelState::Done,
        PanelState::Asking,
        PanelState::Refused,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PanelState::Idle => "idle",
            PanelState::Listening => "listening",
            PanelState::Thinking => "thinking",
            PanelState::Acting => "acting",
            PanelState::Done => "done",
            PanelState::Asking => "asking",
            PanelState::Refused => "refused",
        }
    }
}

pub struct Desk {
    pub look: Look,
    pub scale: f32,
    pub frame: Frame,
    wallpaper: Frame,
    pub windows: Vec<Window>,
    pub shadow: Shadow,
    pub topbar: Rect,
    pub panel: Rect,
    panel_layer: Frame,
    /// The sheet's layer and where it is now, when it shows.
    pub sheet: Option<(Frame, Rect)>,
}

/// px for `logical` at `scale`.
pub fn px(logical: f32, scale: f32) -> i32 {
    libm::roundf(logical * scale) as i32
}

fn opaque(c: u32) -> u32 {
    c | 0xFF00_0000
}

/// Rounds a layer's corners: each corner pixel's alpha is its coverage of a
/// quarter disc of radius `r`, premultiplied.
fn round_corners(f: &mut Frame, r: i32) {
    let rf = r as f32;
    for cy in 0..r {
        for cx in 0..r {
            let (dx, dy) = (rf - cx as f32 - 0.5, rf - cy as f32 - 0.5);
            let d = libm::sqrtf(dx * dx + dy * dy);
            let a = ((rf - d + 0.5).clamp(0.0, 1.0) * 255.0) as u32;
            for (x, y) in [
                (cx, cy),
                (f.w - 1 - cx, cy),
                (cx, f.h - 1 - cy),
                (f.w - 1 - cx, f.h - 1 - cy),
            ] {
                let p = &mut f.px[(y * f.w + x) as usize];
                *p = crate::frame::tint(*p & 0x00FF_FFFF, a, 0) | (a << 24);
            }
        }
    }
}

/// A 1 px frame just inside `r`.
fn frame_line(f: &mut Frame, r: Rect, color: u32) {
    f.fill(Rect::new(r.x, r.y, r.w, 1), color);
    f.fill(Rect::new(r.x, r.bottom() - 1, r.w, 1), color);
    f.fill(Rect::new(r.x, r.y, 1, r.h), color);
    f.fill(Rect::new(r.right() - 1, r.y, 1, r.h), color);
}

impl Desk {
    pub fn new(w: i32, h: i32, look: Look, scale: f32, text: &mut Text) -> Desk {
        let mut wallpaper = Frame::new(w, h);
        // A quiet vertical ramp from `desk`, so damage is visible in screendumps.
        for y in 0..h {
            let t = (y * 12 / h.max(1)) as u32;
            let c = if look.dark {
                look.desk + t * 0x010101
            } else {
                look.desk - t * 0x010101
            };
            wallpaper.fill(Rect::new(0, y, w, 1), c);
        }
        let topbar = Rect::new(0, 0, w, px(28.0, scale));
        let panel = Rect::new(0, h - px(36.0, scale), w, px(36.0, scale));
        let mut desk = Desk {
            look,
            scale,
            frame: Frame::new(w, h),
            wallpaper,
            windows: Vec::new(),
            shadow: Shadow::new(px(16.0, scale), 120, 0x000000),
            topbar,
            panel,
            panel_layer: Frame::new(w, panel.h),
            sheet: None,
        };
        desk.draw_topbar(text);
        desk.set_panel(text, PanelState::Idle);
        desk
    }

    fn draw_topbar(&mut self, text: &mut Text) {
        let (look, s, bar) = (self.look, self.scale, self.topbar);
        self.wallpaper.fill(bar, look.window);
        self.wallpaper
            .fill(Rect::new(0, bar.bottom() - 1, bar.w, 1), look.line);
        let base = bar.y + px(19.0, s);
        let mono = Style {
            family: Family::Mono,
            size: 12.0 * s,
            weight: 500,
        };
        let x = text.draw(
            &mut self.wallpaper,
            bar,
            px(12.0, s) as f32,
            base,
            Style {
                weight: 700,
                ..mono
            },
            look.ai,
            "Kernel AI",
        );
        let mut at = px(12.0, s) as f32 + x + px(24.0, s) as f32;
        for tab in ["notes", "terminal", "files"] {
            at += text.draw(&mut self.wallpaper, bar, at, base, mono, look.muted, tab)
                + px(16.0, s) as f32;
        }
        let right = "model ready  cpu 4%  mem 1.2 GiB";
        let (_, wd) = text.shape(mono, right);
        text.draw(
            &mut self.wallpaper,
            bar,
            bar.w as f32 - wd - px(12.0, s) as f32,
            base,
            mono,
            look.dim,
            right,
        );
    }

    /// A window with chrome and `content` drawn below the title bar; `alpha`
    /// under 255 makes its body translucent (scene (a)'s alpha window).
    pub fn add_window(
        &mut self,
        text: &mut Text,
        rect: Rect,
        title: &'static str,
        focused: bool,
        alpha: u32,
        content: &mut dyn FnMut(&mut Frame, &mut Text, Rect),
    ) {
        let (look, s) = (self.look, self.scale);
        let mut layer = Frame::new(rect.w, rect.h);
        let body = if alpha >= 255 {
            opaque(look.window)
        } else {
            crate::frame::tint(look.window, alpha, 0) | (alpha << 24)
        };
        layer.fill(layer.bounds(), body);
        let bar = Rect::new(0, 0, rect.w, px(32.0, s));
        layer.fill(bar, opaque(look.window));
        layer.fill(Rect::new(0, bar.bottom() - 1, rect.w, 1), opaque(look.line));
        let mono_title = Style {
            family: Family::Mono,
            size: 12.0 * s,
            weight: 500,
        };
        let title_color = if focused { look.text } else { look.dim };
        text.draw(
            &mut layer,
            bar,
            px(12.0, s) as f32,
            px(21.0, s),
            mono_title,
            title_color,
            title,
        );
        // Three 28x28 controls, their 16 px marks drawn as strokes.
        let c = px(28.0, s);
        for (i, mark) in ["-", "[]", "x"].iter().enumerate() {
            let x = rect.w - (3 - i as i32) * c - px(4.0, s);
            let mid = Rect::new(
                x + c / 2 - px(6.0, s),
                px(16.0, s),
                px(12.0, s),
                1.max(px(1.5, s)),
            );
            match *mark {
                "-" => layer.fill(mid, opaque(look.muted)),
                "[]" => frame_line(
                    &mut layer,
                    Rect::new(
                        x + c / 2 - px(5.0, s),
                        px(11.0, s),
                        px(10.0, s),
                        px(10.0, s),
                    ),
                    opaque(look.muted),
                ),
                _ => {
                    for d in 0..px(10.0, s) {
                        let at = |dx: i32| {
                            Rect::new(
                                x + c / 2 - px(5.0, s) + dx,
                                px(11.0, s) + d,
                                1.max(px(1.5, s)),
                                1,
                            )
                        };
                        layer.fill(at(d), opaque(look.muted));
                        layer.fill(at(px(10.0, s) - 1 - d), opaque(look.muted));
                    }
                }
            }
        }
        let inner = Rect::new(1, bar.bottom(), rect.w - 2, rect.h - bar.h - 1);
        content(&mut layer, text, inner);
        let edge = if focused { look.line_strong } else { look.line };
        let bounds = layer.bounds();
        frame_line(&mut layer, bounds, opaque(edge));
        round_corners(&mut layer, px(6.0, s));
        self.windows.push(Window {
            opaque: alpha >= 255,
            rect,
            layer,
            title,
            focused,
        });
    }

    /// Redraws the content of window `i`'s body (scene (c)) and returns its rect.
    pub fn redraw(
        &mut self,
        i: usize,
        text: &mut Text,
        content: &mut dyn FnMut(&mut Frame, &mut Text, Rect),
    ) -> Rect {
        let (look, s) = (self.look, self.scale);
        let w = &mut self.windows[i];
        let inner = Rect::new(1, px(32.0, s), w.rect.w - 2, w.rect.h - px(32.0, s) - 1);
        w.layer.fill(inner, opaque(look.window));
        content(&mut w.layer, text, inner);
        inner.offset(w.rect.x, w.rect.y)
    }

    pub fn set_panel(&mut self, text: &mut Text, state: PanelState) {
        let (look, s) = (self.look, self.scale);
        let f = &mut self.panel_layer;
        let all = f.bounds();
        f.fill(all, opaque(look.window));
        f.fill(Rect::new(0, 0, all.w, 1), opaque(look.line));
        if state == PanelState::Listening {
            f.fill(Rect::new(0, 0, all.w, px(2.0, s)), opaque(look.ai_mark));
        }
        let base = px(23.0, s);
        let mono = Style {
            family: Family::Mono,
            size: 14.0 * s,
            weight: 400,
        };
        let x = px(12.0, s) as f32;
        let w = text.draw(
            f,
            all,
            x,
            base,
            Style {
                weight: 700,
                ..mono
            },
            look.ai,
            "kai>",
        );
        let at = x + w + px(10.0, s) as f32;
        let (input, input_color) = match state {
            PanelState::Idle => ("ask, or type Rhai", look.dim),
            PanelState::Listening => ("make a note called groceries", look.text),
            _ => ("make a note called groceries", look.muted),
        };
        let typed = text.draw(f, all, at, base, mono, input_color, input);
        if state == PanelState::Listening {
            f.fill(
                Rect::new((at + typed) as i32 + 2, px(9.0, s), px(8.0, s), px(18.0, s)),
                opaque(look.ai_mark),
            );
        }
        let mut right = String::new();
        let (state_text, color) = match state {
            PanelState::Idle => ("model ready", look.dim),
            PanelState::Listening => ("Enter runs", look.dim),
            PanelState::Thinking => ("... thinking 4s  Esc stops", look.ai),
            PanelState::Acting => (
                "notes_create(\"groceries\")  Save (button) in Notes",
                look.ai,
            ),
            PanelState::Done => ("Done: created groceries. undo  show", look.ok),
            PanelState::Asking => ("waiting for you", look.ask),
            PanelState::Refused => ("Refused: notes is not allowed to send mail", look.error),
        };
        right.push_str(state_text);
        let (_, wd) = text.shape(mono, &right);
        text.draw(
            f,
            all,
            all.w as f32 - wd - px(12.0, s) as f32,
            base,
            mono,
            color,
            &right,
        );
        self.sheet = match state {
            PanelState::Asking => Some(self.ask_sheet(text)),
            _ => None,
        };
    }

    /// The ask sheet: `raised`, at most 960 px wide and half the screen tall,
    /// with a 3 px `ask` bar, the call in mono, the reason, Run (y) and Don't (n).
    pub fn ask_sheet(&self, text: &mut Text) -> (Frame, Rect) {
        let (look, s) = (self.look, self.scale);
        let w = px(960.0, s).min(self.frame.w - px(32.0, s));
        let h = px(220.0, s).min(self.frame.h / 2);
        let mut f = Frame::new(w, h);
        f.fill(f.bounds(), opaque(look.raised));
        f.fill(Rect::new(0, 0, w, px(3.0, s)), opaque(look.ask));
        let body = Style {
            family: Family::Sans,
            size: 14.0 * s,
            weight: 400,
        };
        let mono = Style {
            family: Family::Mono,
            size: 14.0 * s,
            weight: 400,
        };
        let all = f.bounds();
        let x = px(20.0, s) as f32;
        text.draw(
            &mut f,
            all,
            x,
            px(36.0, s),
            Style {
                size: 18.0 * s,
                weight: 600,
                ..body
            },
            look.text,
            "Allow this?",
        );
        text.draw(
            &mut f,
            all,
            x,
            px(72.0, s),
            mono,
            look.text,
            "mail_send(to: \"pat@example.com\", subject: \"groceries\")",
        );
        text.draw(
            &mut f,
            all,
            x,
            px(104.0, s),
            body,
            look.muted,
            "It sends a message from your account. Nothing undoes it.",
        );
        text.draw(
            &mut f,
            all,
            x,
            px(136.0, s),
            body,
            look.dim,
            "Expires in 2:00",
        );
        let by = px(160.0, s);
        f.fill(
            Rect::new(px(20.0, s), by, px(120.0, s), px(36.0, s)),
            opaque(look.ask),
        );
        text.draw(
            &mut f,
            all,
            x + px(18.0, s) as f32,
            by + px(24.0, s),
            Style {
                weight: 600,
                ..body
            },
            look.raised,
            "Run (y)",
        );
        frame_line(
            &mut f,
            Rect::new(px(156.0, s), by, px(130.0, s), px(36.0, s)),
            opaque(look.line_strong),
        );
        text.draw(
            &mut f,
            all,
            x + px(156.0, s) as f32,
            by + px(24.0, s),
            Style {
                weight: 600,
                ..body
            },
            look.text,
            "Don't (n)",
        );
        frame_line(&mut f, all, opaque(look.line_strong));
        let at = Rect::new((self.frame.w - w) / 2, self.panel.y - h - px(8.0, s), w, h);
        (f, at)
    }

    /// Redraws `damage`: the wallpaper and top bar, every window bottom first
    /// with the focused one's shadow, the panel and the sheet.
    pub fn compose(&mut self, damage: Rect) {
        let damage = damage.intersect(&self.frame.bounds());
        if damage.is_empty() {
            return;
        }
        // Occlusion: the topmost opaque window that holds all of `damage` hides
        // everything below it except in its four rounded corner squares, so the
        // layers below are drawn only there.
        let radius = px(6.0, self.scale);
        let first = self
            .windows
            .iter()
            .rposition(|w| w.opaque && w.rect.intersect(&damage) == damage);
        match first {
            None => self.frame.draw(&self.wallpaper, (0, 0), damage, false),
            Some(top) => {
                let r = self.windows[top].rect;
                let corners = [
                    Rect::new(r.x, r.y, radius, radius),
                    Rect::new(r.right() - radius, r.y, radius, radius),
                    Rect::new(r.x, r.bottom() - radius, radius, radius),
                    Rect::new(r.right() - radius, r.bottom() - radius, radius, radius),
                ];
                for corner in corners {
                    let part = corner.intersect(&damage);
                    if part.is_empty() {
                        continue;
                    }
                    self.frame.draw(&self.wallpaper, (0, 0), part, false);
                    for w in &self.windows[..top] {
                        if w.focused {
                            self.shadow.draw(&mut self.frame, w.rect, part);
                        }
                        self.frame.draw_window(
                            &w.layer,
                            (w.rect.x, w.rect.y),
                            part,
                            radius,
                            w.opaque,
                        );
                    }
                }
            }
        }
        for w in &self.windows[first.unwrap_or(0)..] {
            if w.focused {
                self.shadow.draw(&mut self.frame, w.rect, damage);
            }
            self.frame
                .draw_window(&w.layer, (w.rect.x, w.rect.y), damage, radius, w.opaque);
        }
        self.frame
            .draw(&self.panel_layer, (0, self.panel.y), damage, false);
        if let Some((layer, at)) = &self.sheet {
            self.shadow.draw(&mut self.frame, *at, damage);
            self.frame.draw(layer, (at.x, at.y), damage, false);
        }
    }

    /// The rectangle a window's move from `from` to `to` damages, shadow
    /// included.
    pub fn move_damage(&self, from: Rect, to: Rect) -> Rect {
        from.grow(self.shadow.m).union(&to.grow(self.shadow.m))
    }
}
