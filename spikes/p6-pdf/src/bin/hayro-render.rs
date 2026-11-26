//! Renders up to `max_pages` pages of a PDF with hayro 0.9 to
//! `<out>/page-<n>.png` at `dpi`, on a white background, as `pdftoppm -r`
//! does. One line a page on stdout, `page <n> <ms> <width> <height>`, then
//! `status ok`, or `status load <why>` or `status panic <page>`: a page that
//! panics is reported, not fatal, so the rest still render.
//!
//! Non-embedded CJK fonts fall back to the Noto Sans CJK collection at
//! `$KAI_CJK_FONT` when it is set, as poppler falls back to fontconfig's;
//! other fonts to hayro's embedded standard 14.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::font::FontQuery;
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

fn settings() -> InterpreterSettings {
    let cjk = std::env::var("KAI_CJK_FONT")
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .map(Arc::new);
    InterpreterSettings {
        font_resolver: Arc::new(move |query| match query {
            FontQuery::Standard(s) => Some(s.get_font_data()),
            FontQuery::Fallback(f) => {
                // NotoSansCJK-Regular.ttc: JP, KR, SC, TC.
                let index = f
                    .character_collection
                    .as_ref()
                    .and_then(|cc| match cc.family {
                        CidFamily::AdobeJapan1 => Some(0),
                        CidFamily::AdobeKorea1 => Some(1),
                        CidFamily::AdobeGB1 => Some(2),
                        CidFamily::AdobeCNS1 => Some(3),
                        _ => None,
                    });
                match (index, &cjk) {
                    (Some(index), Some(data)) => Some((data.clone(), index)),
                    _ => Some(f.pick_standard_font().get_font_data()),
                }
            }
        }),
        ..Default::default()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, out, dpi, max_pages] = &args[..] else {
        eprintln!("usage: hayro-render <in.pdf> <out-dir> <dpi> <max-pages>");
        std::process::exit(2);
    };
    let dpi: f32 = dpi.parse().expect("a dpi");
    let max_pages: usize = max_pages.parse().expect("a page count");
    std::fs::create_dir_all(out).expect("the out dir");
    let bytes = std::fs::read(input).expect("the PDF");
    let started = Instant::now();
    let pdf = match Pdf::new(bytes) {
        Ok(pdf) => pdf,
        Err(e) => {
            println!("status load {e:?}");
            return;
        }
    };
    println!(
        "loaded {} ms, {} pages",
        started.elapsed().as_millis(),
        pdf.pages().len()
    );
    let interpreter = settings();
    let cache = RenderCache::new();
    let pixmap = PixmapSettings {
        x_scale: dpi / 72.0,
        y_scale: dpi / 72.0,
        bg_color: WHITE,
    };
    for (n, page) in pdf.pages().iter().take(max_pages).enumerate() {
        let n = n + 1;
        let at = Instant::now();
        let rendered = catch_unwind(AssertUnwindSafe(|| {
            render(
                page,
                &cache,
                &interpreter,
                &RenderSettings::default(),
                &pixmap,
            )
        }));
        let Ok(rendered) = rendered else {
            println!("status panic {n}");
            return;
        };
        let ms = at.elapsed().as_secs_f64() * 1000.0;
        let (w, h) = (rendered.width(), rendered.height());
        let png = rendered.into_png().expect("a PNG");
        std::fs::write(format!("{out}/page-{n}.png"), png).expect("the PNG written");
        println!("page {n} {ms:.1} {w} {h}");
    }
    println!("status ok");
}
