//! S14-1's engine: Blitz (blitz-html, blitz-dom, blitz-paint) painting a page
//! with anyrender_vello_cpu, the same code built natively and for
//! `wasm32-wasip1`/`wasm32-wasip2`. Each phase is timed inside the program,
//! with the clock its target gives (WASI's monotonic clock in a guest).

pub mod fixtures;

use anyrender::ImageRenderer;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, StyleThreading};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use std::time::Instant;

/// The one font: Inter, from the repo's assets (SIL OFL 1.1). No system fonts,
/// so native and guest runs lay text out the same.
pub static FONT: &[u8] = include_bytes!("../../../../assets/fonts/InterVariable.ttf");

/// The viewport every fixture is drawn at.
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 800;

/// One page through the engine, in microseconds per phase.
#[derive(Clone, Copy, Debug, Default)]
pub struct Report {
    /// HTML to a DOM, stylesheets included (`HtmlDocument::from_html`).
    pub parse_us: u64,
    /// Stylo's cascade, box construction and Taffy's layout (`resolve`).
    pub layout_us: u64,
    /// The display list and vello_cpu's raster into an RGBA8 buffer.
    pub paint_us: u64,
    /// The sum: the page's first paint.
    pub total_us: u64,
    /// FNV-1a over the pixels, so every runtime is seen to draw the same.
    pub pixels: u64,
}

fn us(since: Instant) -> u64 {
    since.elapsed().as_micros() as u64
}

/// Parses, lays out and paints `html` at [`WIDTH`]x[`HEIGHT`].
pub fn render(html: &str) -> Report {
    let start = Instant::now();
    let config = DocumentConfig {
        viewport: Some(Viewport::new(WIDTH, HEIGHT, 1.0, ColorScheme::Light)),
        font_ctx: Some(blitz_dom::build_single_font_ctx(FONT)),
        style_threading: StyleThreading::Sequential,
        ..DocumentConfig::default()
    };
    let mut doc = HtmlDocument::from_html(html, config).into_inner();
    let parse_us = us(start);
    let t = Instant::now();
    doc.resolve(0.0);
    let layout_us = us(t);
    let t = Instant::now();
    let mut renderer = VelloCpuImageRenderer::new(WIDTH, HEIGHT);
    let mut buffer = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    renderer.render(
        |scene| blitz_paint::paint_scene(scene, &mut doc, 1.0, WIDTH, HEIGHT, 0, 0),
        &mut buffer,
    );
    let paint_us = us(t);
    if let Some(path) = std::env::var_os("S14_RAW") {
        let _ = std::fs::write(path, &buffer);
    }
    Report {
        parse_us,
        layout_us,
        paint_us,
        total_us: parse_us + layout_us + paint_us,
        pixels: fnv(&buffer),
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}
