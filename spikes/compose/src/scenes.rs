//! S05-1's scenes, each 300 frames: (a) a full composite with wallpaper and four
//! windows, one with alpha; (b) an 800x600 window dragged 200 px with its
//! shadow; (c) a 160x45 page of terminal text, cold and warm; (d) the ask sheet's
//! spring, 17 frames, repeated; (f) is (b) under load, run by the caller. (e)'s
//! client draws with [`client_draw`]. Render and flush are timed apart.

use crate::desk::{Desk, PanelState, px};
use crate::frame::{Frame, Rect};
use crate::look::Look;
use crate::text::{Family, Style, Text};
use crate::vello::VelloDesk;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const FRAMES: usize = 300;
/// Cold text pages: each starts from an empty atlas.
pub const COLD_PAGES: usize = 10;
pub const SPRING_FRAMES: usize = 17;

/// Where frames go and what time it is.
pub trait Screen {
    /// Puts `damage` of `frame` on screen: the rows copied to the scanout
    /// buffer, then flushed.
    fn present(&mut self, frame: &Frame, damage: &[Rect]);
    /// The TSC.
    fn now(&self) -> u64;
}

#[derive(Default)]
pub struct Timings {
    pub render: Vec<u64>,
    pub flush: Vec<u64>,
}

/// The 160x45 page's lines: shell-like text, every printable ASCII byte used.
pub fn terminal_line(n: usize) -> String {
    let words = [
        "kai>",
        "ls",
        "/home/user",
        "notes.txt",
        "12.4 KiB",
        "done:",
        "created",
        "groceries",
        "undo",
        "fn",
        "main()",
        "{",
        "}",
        "0x7f3a",
        "[ok]",
        "build",
        "--release",
        "warning:",
        "#[test]",
        "=>",
    ];
    let mut line = format!("{n:>3} ");
    let mut i = n;
    while line.len() < 160 {
        line.push_str(words[i % words.len()]);
        line.push(' ');
        i = i.wrapping_mul(7).wrapping_add(3);
    }
    line.truncate(160);
    line
}

fn mono(scale: f32) -> Style {
    Style {
        family: Family::Mono,
        size: 14.0 * scale,
        weight: 400,
    }
}

fn notes_content(look: Look, scale: f32) -> impl FnMut(&mut Frame, &mut Text, Rect) {
    move |f: &mut Frame, text: &mut Text, inner: Rect| {
        let body = Style {
            family: Family::Sans,
            size: 14.0 * scale,
            weight: 400,
        };
        let head = Style {
            size: 18.0 * scale,
            weight: 600,
            ..body
        };
        let x = (inner.x + px(16.0, scale)) as f32;
        let mut y = inner.y + px(32.0, scale);
        text.draw(f, inner, x, y, head, look.text, "Groceries");
        for line in [
            "Milk, eggs and bread for the week.",
            "Ask Pat about the farmers' market on Saturday.",
            "Coffee beans: the dark roast, 500 g.",
        ] {
            y += px(24.0, scale);
            text.draw(f, inner, x, y, body, look.muted, line);
        }
    }
}

fn terminal_content(
    look: Look,
    scale: f32,
    rows: usize,
) -> impl FnMut(&mut Frame, &mut Text, Rect) {
    move |f: &mut Frame, text: &mut Text, inner: Rect| {
        let style = mono(scale);
        for row in 0..rows {
            let y = inner.y + px(4.0, scale) + px(20.0, scale) * (row as i32 + 1) - px(5.0, scale);
            let color = if row % 9 == 0 { look.ai } else { look.text };
            text.draw(
                f,
                inner,
                (inner.x + px(8.0, scale)) as f32,
                y,
                style,
                color,
                &terminal_line(row),
            );
        }
    }
}

/// The desktop the scenes share: notes (focused), a terminal, files and a
/// translucent window, all within the screen.
pub fn desk(w: i32, h: i32, look: Look, scale: f32, text: &mut Text) -> Desk {
    let mut d = Desk::new(w, h, look, scale, text);
    let s = scale;
    let top = d.topbar.bottom() + px(24.0, s);
    let fit = |r: Rect| r.intersect(&Rect::new(0, 0, w, h - px(36.0, s)));
    d.add_window(
        text,
        fit(Rect::new(px(40.0, s), top, px(640.0, s), px(420.0, s))),
        "files",
        false,
        255,
        &mut |_, _, _| {},
    );
    d.add_window(
        text,
        fit(Rect::new(
            px(320.0, s),
            top + px(120.0, s),
            px(720.0, s),
            px(400.0, s),
        )),
        "terminal",
        false,
        255,
        &mut terminal_content(look, s, 18),
    );
    d.add_window(
        text,
        fit(Rect::new(
            px(760.0, s),
            top + px(40.0, s),
            px(480.0, s),
            px(320.0, s),
        )),
        "glass",
        false,
        200,
        &mut |_, _, _| {},
    );
    d.add_window(
        text,
        fit(Rect::new(
            px(200.0, s),
            top + px(220.0, s),
            px(800.0, s),
            px(600.0, s),
        )),
        "Notes",
        true,
        255,
        &mut notes_content(look, s),
    );
    d
}

/// (a): the whole screen composed and flushed, 300 times.
pub fn full(d: &mut Desk, screen: &mut dyn Screen) -> Timings {
    let mut t = Timings::default();
    let all = d.frame.bounds();
    for _ in 0..FRAMES {
        let t0 = screen.now();
        d.compose(all);
        let t1 = screen.now();
        screen.present(&d.frame, &[all]);
        t.render.push(t1 - t0);
        t.flush.push(screen.now() - t1);
    }
    t
}

/// (b) and (f): the focused 800x600 window dragged 200 px right and back, 300
/// frames, each damaging the union of where it and its shadow were and are.
pub fn drag(d: &mut Desk, screen: &mut dyn Screen) -> Timings {
    let mut t = Timings::default();
    let i = d.windows.len() - 1;
    let start = d.windows[i].rect;
    let reach = px(200.0, d.scale);
    for n in 0..FRAMES {
        let half = FRAMES / 2;
        let k = if n < half { n } else { FRAMES - n };
        let to = start.offset(reach * k as i32 / half as i32, 0);
        let from = d.windows[i].rect;
        d.windows[i].rect = to;
        let damage = d.move_damage(from, to);
        let t0 = screen.now();
        d.compose(damage);
        let t1 = screen.now();
        screen.present(&d.frame, &[damage]);
        t.render.push(t1 - t0);
        t.flush.push(screen.now() - t1);
    }
    d.windows[i].rect = start;
    t
}

/// (c): a 160x45 page of terminal text in a window of its size, drawn and
/// composed: [`COLD_PAGES`] from an empty atlas, then 300 warm.
pub fn text_page(d: &mut Desk, text: &mut Text, screen: &mut dyn Screen) -> (Timings, Timings) {
    let s = d.scale;
    let (_, adv) = text.shape(mono(s), &terminal_line(0));
    let w = (adv as i32 + px(18.0, s)).min(d.frame.w);
    let h = (px(32.0, s) + px(20.0, s) * 45 + px(9.0, s)).min(d.frame.h - d.topbar.h - d.panel.h);
    let rect = Rect::new((d.frame.w - w) / 2, d.topbar.bottom(), w, h);
    let look = d.look;
    d.add_window(text, rect, "terminal 160x45", true, 255, &mut |_, _, _| {});
    let i = d.windows.len() - 1;
    let mut page = terminal_content(look, s, 45);
    let mut run = |d: &mut Desk, text: &mut Text, cold: bool| {
        if cold {
            text.clear();
        }
        let t0 = screen.now();
        let damage = d.redraw(i, text, &mut page);
        d.compose(damage);
        let t1 = screen.now();
        screen.present(&d.frame, &[damage]);
        (t1 - t0, screen.now() - t1)
    };
    let (mut cold, mut warm) = (Timings::default(), Timings::default());
    for _ in 0..COLD_PAGES {
        let (r, f) = run(d, text, true);
        cold.render.push(r);
        cold.flush.push(f);
    }
    for _ in 0..FRAMES {
        let (r, f) = run(d, text, false);
        warm.render.push(r);
        warm.flush.push(f);
    }
    d.windows.pop();
    d.compose(d.frame.bounds());
    (cold, warm)
}

/// (d): the ask sheet rising on ADR 0005's spring (stiffness 503.6, damping
/// 44.9, translate only) over 17 frames at 60 Hz, repeated to 300 frames.
pub fn sheet(d: &mut Desk, text: &mut Text, screen: &mut dyn Screen) -> Timings {
    let mut t = Timings::default();
    d.set_panel(text, PanelState::Asking);
    let Some((_, target)) = d.sheet else {
        return t;
    };
    let travel = (d.frame.h - target.y) as f32;
    let dt = 1.0 / 60.0;
    let mut frames = 0;
    while frames < FRAMES {
        let (mut x, mut v) = (travel, 0.0f32);
        let mut at = target.offset(0, travel as i32);
        for _ in 0..SPRING_FRAMES {
            let a = -503.6 * x - 44.9 * v;
            v += a * dt;
            x += v * dt;
            let next = target.offset(0, libm::roundf(x) as i32);
            if let Some((_, r)) = &mut d.sheet {
                *r = next;
            }
            let damage = at.grow(d.shadow.m).union(&next.grow(d.shadow.m));
            at = next;
            let t0 = screen.now();
            d.compose(damage);
            let t1 = screen.now();
            screen.present(&d.frame, &[damage]);
            t.render.push(t1 - t0);
            t.flush.push(screen.now() - t1);
            frames += 1;
        }
    }
    d.set_panel(text, PanelState::Idle);
    d.compose(d.frame.bounds());
    t
}

/// (a) and (b) on vello_cpu: render only.
pub fn vello(d: &mut Desk, screen: &mut dyn Screen) -> (Vec<u64>, Vec<u64>) {
    let mut v = VelloDesk::new(d);
    let all = d.frame.bounds();
    let mut full = Vec::new();
    for _ in 0..FRAMES {
        let t0 = screen.now();
        v.render(d, all);
        full.push(screen.now() - t0);
    }
    let i = d.windows.len() - 1;
    let start = d.windows[i].rect;
    let reach = px(200.0, d.scale);
    let mut drag = Vec::new();
    for n in 0..FRAMES {
        let half = FRAMES / 2;
        let k = if n < half { n } else { FRAMES - n };
        let to = start.offset(reach * k as i32 / half as i32, 0);
        let damage = d.move_damage(d.windows[i].rect, to);
        d.windows[i].rect = to;
        let t0 = screen.now();
        v.render(d, damage);
        drag.push(screen.now() - t0);
    }
    d.windows[i].rect = start;
    (full, drag)
}

/// (e)'s client: redraws its 600x400 region for key `n`, a line of text that
/// names the key on a fresh background.
pub fn client_draw(f: &mut Frame, text: &mut Text, look: Look, scale: f32, n: u32) {
    let all = f.bounds();
    f.fill(all, look.window | 0xFF00_0000);
    let body = Style {
        family: Family::Sans,
        size: 14.0 * scale,
        weight: 400,
    };
    for row in 0..12 {
        let line = format!("key {n}: line {row} of the client's text, redrawn on every key");
        text.draw(
            f,
            all,
            px(16.0, scale) as f32,
            px(28.0, scale) * (row + 1),
            body,
            look.text,
            &line,
        );
    }
}

/// ADR 0005 §9's default scale for a screen `width` px wide.
pub fn default_scale(width: i32) -> f32 {
    if width <= 1920 {
        1.0
    } else if width <= 2560 {
        1.25
    } else {
        2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::{DARK, LIGHT};

    struct Clock(u64, usize);

    impl Screen for Clock {
        fn present(&mut self, _: &Frame, damage: &[Rect]) {
            self.1 += damage.len();
        }
        fn now(&self) -> u64 {
            self.0
        }
    }

    #[test]
    fn every_scene_draws_and_times_each_frame() {
        for look in [DARK, LIGHT] {
            let mut text = Text::new(crate::INTER, crate::MONO, look.dark);
            let mut d = desk(1280, 800, look, 1.0, &mut text);
            let mut screen = Clock(0, 0);
            assert_eq!(full(&mut d, &mut screen).render.len(), FRAMES);
            assert!(
                d.frame.px.iter().any(|&p| p == look.window),
                "windows are drawn"
            );
            assert!(
                d.frame.px.iter().any(|&p| p == look.ai),
                "the brand and kai> are ember"
            );
            assert_eq!(drag(&mut d, &mut screen).flush.len(), FRAMES);
            let (cold, warm) = text_page(&mut d, &mut text, &mut screen);
            assert_eq!((cold.render.len(), warm.render.len()), (COLD_PAGES, FRAMES));
            assert!(sheet(&mut d, &mut text, &mut screen).render.len() >= FRAMES);
            let (vf, vd) = vello(&mut d, &mut screen);
            assert_eq!((vf.len(), vd.len()), (FRAMES, FRAMES));
        }
    }

    #[test]
    fn a_text_page_line_is_160_columns() {
        assert_eq!(terminal_line(44).chars().count(), 160);
    }
}

#[cfg(test)]
mod dump {
    use super::*;
    use crate::look::{DARK, LIGHT};
    use std::io::Write;

    /// `KAI_DUMP=dir cargo test -p spike-compose dump -- --ignored` writes each
    /// panel state as a PPM, for a look on the host.
    #[test]
    #[ignore]
    fn dump_panel_states() {
        let dir = std::env::var("KAI_DUMP").unwrap_or_else(|_| ".".into());
        for look in [DARK, LIGHT] {
            let mut text = Text::new(crate::INTER, crate::MONO, look.dark);
            let mut d = desk(1920, 1080, look, 1.0, &mut text);
            for state in PanelState::ALL {
                d.set_panel(&mut text, state);
                d.compose(d.frame.bounds());
                let name = format!(
                    "{dir}/{}-{}.ppm",
                    if look.dark { "dark" } else { "light" },
                    state.name()
                );
                let mut out = std::fs::File::create(&name).unwrap();
                write!(out, "P6\n{} {}\n255\n", d.frame.w, d.frame.h).unwrap();
                let bytes: Vec<u8> = d
                    .frame
                    .px
                    .iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
                    .collect();
                out.write_all(&bytes).unwrap();
            }
        }
    }
}
