//! The look, pinned: scenes painted on the host, as digests. With
//! `DESK_PREVIEW=dir` each scene is also written there as a PPM to look at.

use crate::desk::Desk;
use crate::geom::{Rect, Size};
use crate::paint::{Bar, Painter, Scene};
use crate::panel::{Panel, State};
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::{Fonts, Image};
use sha2::{Digest, Sha256};

const FONTS: Fonts = Fonts {
    sans: include_bytes!("../../assets/fonts/InterVariable.ttf"),
    mono: include_bytes!("../../assets/fonts/JetBrainsMono-Variable.ttf"),
};

const BAR: Bar = Bar {
    model: true,
    mem_used_mib: 412,
    mem_total_mib: 1763,
};

fn paint(name: &str, desk: &Desk, w: i32, h: i32) -> String {
    paint_with(name, desk, &Panel::new(), w, h)
}

fn paint_with(name: &str, desk: &Desk, panel: &Panel, w: i32, h: i32) -> String {
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let scene = Scene {
        desk,
        bar: &BAR,
        panel,
        now_ms: 4_200,
    };
    painter.paint(
        &mut canvas,
        Rect::new(0, 0, w, h),
        &scene,
        &mut |_, _, _| false,
    );
    painter.cursor(&mut canvas, Rect::new(0, 0, w, h), desk.pointer());
    if let Ok(dir) = std::env::var("DESK_PREVIEW") {
        let mut ppm = std::format!("P6\n{w} {h}\n255\n").into_bytes();
        for p in &image.px {
            ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        std::fs::write(std::format!("{dir}/{name}.ppm"), ppm).expect("the preview is written");
    }
    let bytes: Vec<u8> = image.px.iter().flat_map(|p| p.to_le_bytes()).collect();
    Sha256::digest(&bytes)
        .iter()
        .map(|b| std::format!("{b:02x}"))
        .collect()
}

#[test]
fn an_empty_desk_paints_the_same_every_time() {
    let desk = Desk::new(1280, 800);
    let first = paint("empty", &desk, 1280, 800);
    assert_eq!(first, paint("empty", &desk, 1280, 800));
}

#[test]
fn two_windows_paint() {
    let mut desk = Desk::new(1280, 800);
    desk.open(1, "notes", "Notes", Size { w: 400, h: 300 }, true);
    desk.open(2, "shell", "Terminal", Size { w: 520, h: 320 }, true);
    paint("two", &desk, 1280, 800);
}

/// `desk-boot`'s golden (xtask/src/cases/kernel.rs): the empty desk at
/// 1280x800 with the cursor in the middle, as `desktop` first draws it on GOP,
/// its RGB bytes hashed with the top bar's stats blanked. The boot's screendump
/// must hash the same.
const DESK_BOOT: &str = "514ded10f7fe4dd26d45737cbd9831ff457c6cf5ff59631ecc042c33342a51bb";

#[test]
fn the_boot_desk_matches_desk_boot_s_golden() {
    let (w, h) = (1280, 800);
    let desk = Desk::new(w, h);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let all = Rect::new(0, 0, w, h);
    let panel = Panel::new();
    let scene = Scene {
        desk: &desk,
        bar: &BAR,
        panel: &panel,
        now_ms: 0,
    };
    painter.paint(&mut canvas, all, &scene, &mut |_, _, _| false);
    painter.cursor(&mut canvas, all, desk.pointer());
    let stats = crate::paint::stats_area(w);
    let mut rgb = Vec::new();
    for (i, p) in image.px.iter().enumerate() {
        let (x, y) = (i as i32 % w, i as i32 / w);
        let masked = x >= stats.x && x < stats.right() && y >= stats.y && y < stats.bottom();
        let p = if masked { 0 } else { *p };
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    let got: String = Sha256::digest(&rgb)
        .iter()
        .map(|b| std::format!("{b:02x}"))
        .collect();
    assert_eq!(
        got, DESK_BOOT,
        "desk-boot's golden changed: update it here and in xtask"
    );
}

#[test]
fn the_panel_paints_each_state() {
    let desk = Desk::new(1280, 800);
    let mut panel = Panel::new();
    paint_with("panel-idle", &desk, &panel, 1280, 800);
    panel.set_focus(true);
    for code in [0x0B, 0x0C] {
        panel.key(code, true, 0);
        panel.key(code, false, 0);
    }
    paint_with("panel-listening", &desk, &panel, 1280, 800);
    panel.set_state(State::Thinking { since_ms: 0 });
    paint_with("panel-thinking", &desk, &panel, 1280, 800);
    panel.set_state(State::Done {
        line: "Done: policy round-robin (was interactive).".into(),
    });
    paint_with("panel-done", &desk, &panel, 1280, 800);
    panel.set_state(State::Refused {
        reason: "Refused: notes is protected".into(),
    });
    paint_with("panel-refused", &desk, &panel, 1280, 800);
}
