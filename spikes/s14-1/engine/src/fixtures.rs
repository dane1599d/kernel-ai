//! The ten fixture pages, made here from a fixed seed so nothing from the web
//! is committed and every run draws the same bytes. `article` is the
//! Wikipedia-shaped page the 400 ms first-paint bar is about: an infobox, a
//! contents box, about 40 sections of paragraphs, a few tables and lists, and
//! a references list, roughly 200 KB of HTML.

use std::fmt::Write;

/// Every fixture's name and HTML, in a fixed order.
pub fn all() -> Vec<(&'static str, String)> {
    vec![
        ("hello", hello()),
        ("article", article()),
        ("table", table()),
        ("flex", flex()),
        ("grid", grid()),
        ("form", form()),
        ("docs", docs()),
        ("rules", rules()),
        ("inline", inline()),
        ("deep", deep()),
    ]
}

/// A word stream from a 64-bit LCG over a small invented vocabulary.
struct Words(u64);

const VOCAB: &[&str] = &[
    "river", "stone", "harbour", "valley", "lantern", "orbit", "copper", "meadow", "signal",
    "archive", "granite", "village", "engine", "season", "compass", "theory", "council", "market",
    "glacier", "bridge", "chapter", "voyage", "garden", "pattern", "tower", "reform", "harvest",
    "canal", "mineral", "province", "the", "of", "and", "in", "was", "a", "to", "by", "with",
    "from", "its", "for", "which", "after", "during", "between", "under", "early", "northern",
    "large", "second", "local", "several", "known", "built", "named", "used",
];

impl Words {
    fn next(&mut self) -> &'static str {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        VOCAB[(self.0 >> 33) as usize % VOCAB.len()]
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next();
        (self.0 >> 17) % n
    }

    fn sentence(&mut self, out: &mut String) {
        let n = 8 + self.below(14);
        for i in 0..n {
            let w = self.next();
            if i == 0 {
                let mut c = w.chars();
                let first = c.next().unwrap().to_ascii_uppercase();
                out.push(first);
                out.push_str(c.as_str());
            } else {
                out.push(' ');
                match self.below(12) {
                    0 => write!(out, "<a href=\"#{w}\">{w}</a>").unwrap(),
                    1 => write!(out, "<i>{w}</i>").unwrap(),
                    _ => out.push_str(w),
                }
            }
        }
        out.push_str(". ");
    }

    fn paragraph(&mut self, out: &mut String) {
        out.push_str("<p>");
        for _ in 0..3 + self.below(5) {
            self.sentence(out);
        }
        if self.below(3) == 0 {
            write!(
                out,
                "<sup><a href=\"#ref{}\">[{}]</a></sup>",
                self.below(80),
                1 + self.below(80)
            )
            .unwrap();
        }
        out.push_str("</p>\n");
    }
}

fn page(title: &str, style: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{title}</title><style>{style}</style></head><body>{body}</body></html>"
    )
}

fn hello() -> String {
    page(
        "hello",
        "body{font-family:sans-serif;margin:40px}",
        "<h1>Hello</h1><p>One paragraph.</p>",
    )
}

const ARTICLE_CSS: &str = "body{font-family:sans-serif;margin:0;color:#202122;background:#fff;line-height:1.6}\
header{border-bottom:1px solid #a2a9b1;padding:8px 24px;display:flex;justify-content:space-between}\
main{max-width:960px;margin:0 auto;padding:16px 24px}\
h1{font-family:serif;font-weight:normal;border-bottom:1px solid #a2a9b1}\
h2{font-family:serif;font-weight:normal;border-bottom:1px solid #a2a9b1;margin-top:1.2em}\
.infobox{float:right;width:22em;margin:0 0 1em 1em;border:1px solid #a2a9b1;background:#f8f9fa;font-size:88%}\
.infobox th{text-align:left;padding:2px 6px}.infobox td{padding:2px 6px}\
.toc{display:inline-block;border:1px solid #a2a9b1;background:#f8f9fa;padding:8px 16px}\
.toc ol{margin:0;padding-left:1.4em}\
table.wikitable{border-collapse:collapse;margin:1em 0}\
table.wikitable td,table.wikitable th{border:1px solid #a2a9b1;padding:4px 8px}\
a{color:#36c;text-decoration:none}.refs{font-size:90%;column-width:30em}";

fn article() -> String {
    let mut w = Words(14);
    let mut b = String::new();
    b.push_str("<header><b>Encyclopedia</b><span>Search</span></header><main>");
    b.push_str("<h1>Copper Valley</h1>");
    b.push_str("<table class=\"infobox\"><tr><th colspan=\"2\">Copper Valley</th></tr>");
    for row in [
        "Region",
        "Area",
        "Population",
        "Founded",
        "Elevation",
        "River",
        "Council",
        "Time zone",
    ] {
        write!(
            b,
            "<tr><th>{row}</th><td>{} {}</td></tr>",
            w.next(),
            w.below(9000)
        )
        .unwrap();
    }
    b.push_str("</table>");
    for _ in 0..3 {
        w.paragraph(&mut b);
    }
    b.push_str("<div class=\"toc\"><b>Contents</b><ol>");
    for s in 0..40 {
        write!(b, "<li><a href=\"#s{s}\">Section {s}</a></li>").unwrap();
    }
    b.push_str("</ol></div>");
    for s in 0..40 {
        write!(
            b,
            "<h2 id=\"s{s}\">Section {s}: {} {}</h2>",
            w.next(),
            w.next()
        )
        .unwrap();
        for _ in 0..2 + w.below(4) {
            w.paragraph(&mut b);
        }
        if s % 7 == 3 {
            b.push_str("<table class=\"wikitable\"><tr><th>Year</th><th>Name</th><th>Count</th><th>Note</th></tr>");
            for r in 0..12 {
                write!(
                    b,
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{} {}</td></tr>",
                    1800 + r * 9,
                    w.next(),
                    w.below(5000),
                    w.next(),
                    w.next()
                )
                .unwrap();
            }
            b.push_str("</table>");
        }
        if s % 5 == 1 {
            b.push_str("<ul>");
            for _ in 0..6 {
                write!(b, "<li>{} {} {}</li>", w.next(), w.next(), w.next()).unwrap();
            }
            b.push_str("</ul>");
        }
    }
    b.push_str("<h2>References</h2><ol class=\"refs\">");
    for r in 0..80 {
        write!(
            b,
            "<li id=\"ref{r}\">{} {} ({}). <i>{} {}</i>.</li>",
            w.next(),
            w.next(),
            1900 + w.below(120),
            w.next(),
            w.next()
        )
        .unwrap();
    }
    b.push_str("</ol></main>");
    page("Copper Valley", ARTICLE_CSS, &b)
}

fn table() -> String {
    let mut w = Words(3);
    let mut b = String::from(
        "<table><thead><tr><th>#</th><th>Name</th><th>Kind</th><th>Size</th><th>Owner</th><th>Note</th></tr></thead><tbody>",
    );
    for r in 0..500 {
        write!(
            b,
            "<tr><td>{r}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{} {}</td></tr>",
            w.next(),
            w.next(),
            w.below(1 << 20),
            w.next(),
            w.next(),
            w.next()
        )
        .unwrap();
    }
    b.push_str("</tbody></table>");
    page(
        "table",
        "body{font-family:sans-serif}table{border-collapse:collapse}td,th{border:1px solid #ccc;padding:2px 6px}tr:nth-child(even){background:#f4f4f4}",
        &b,
    )
}

fn flex() -> String {
    let mut w = Words(5);
    let mut b = String::from("<nav>");
    for i in 0..8 {
        write!(b, "<a href=\"#{i}\">{}</a>", w.next()).unwrap();
    }
    b.push_str("</nav><section class=\"cards\">");
    for _ in 0..60 {
        b.push_str("<div class=\"card\"><h3>");
        b.push_str(w.next());
        b.push_str("</h3>");
        w.paragraph(&mut b);
        b.push_str("<button>Open</button></div>");
    }
    b.push_str("</section>");
    page(
        "flex",
        "body{font-family:sans-serif;margin:0}nav{display:flex;gap:16px;padding:12px;background:#223;}nav a{color:#fff}.cards{display:flex;flex-wrap:wrap;gap:12px;padding:12px}.card{flex:1 1 280px;border:1px solid #ddd;border-radius:8px;padding:12px;display:flex;flex-direction:column}.card button{align-self:flex-end}",
        &b,
    )
}

fn grid() -> String {
    let mut w = Words(6);
    let mut b = String::from("<div class=\"g\">");
    for i in 0..240 {
        write!(
            b,
            "<figure style=\"background:hsl({},50%,70%)\"><figcaption>{} {}</figcaption></figure>",
            i * 7 % 360,
            w.next(),
            i
        )
        .unwrap();
    }
    b.push_str("</div>");
    page(
        "grid",
        "body{font-family:sans-serif}.g{display:grid;grid-template-columns:repeat(auto-fill,minmax(140px,1fr));gap:8px}figure{margin:0;height:100px;display:grid;place-items:end start;border-radius:4px}figcaption{padding:4px;background:rgba(255,255,255,.7)}",
        &b,
    )
}

fn form() -> String {
    let mut w = Words(7);
    let mut b = String::from("<form>");
    for i in 0..40 {
        let label = w.next();
        match i % 4 {
            0 => write!(
                b,
                "<label>{label} <input type=\"text\" value=\"{}\"></label>",
                w.next()
            )
            .unwrap(),
            1 => write!(
                b,
                "<label><input type=\"checkbox\" checked> {label}</label>"
            )
            .unwrap(),
            2 => write!(
                b,
                "<label>{label} <select><option>{}</option><option>{}</option></select></label>",
                w.next(),
                w.next()
            )
            .unwrap(),
            _ => write!(
                b,
                "<label>{label}<textarea>{} {}</textarea></label>",
                w.next(),
                w.next()
            )
            .unwrap(),
        }
    }
    b.push_str("<button type=\"submit\">Send</button></form>");
    page(
        "form",
        "body{font-family:sans-serif}form{display:grid;grid-template-columns:1fr 1fr;gap:10px;max-width:800px}label{display:flex;flex-direction:column}",
        &b,
    )
}

fn docs() -> String {
    let mut w = Words(8);
    let mut b = String::from("<aside><ul>");
    for i in 0..30 {
        write!(
            b,
            "<li>{}<ul><li>{}</li><li>{}</li></ul></li>",
            w.next(),
            w.next(),
            i
        )
        .unwrap();
    }
    b.push_str("</ul></aside><article>");
    for s in 0..15 {
        write!(b, "<h2>{} {s}</h2>", w.next()).unwrap();
        w.paragraph(&mut b);
        b.push_str("<pre><code>");
        for l in 0..8 {
            writeln!(b, "let {} = {}({l});", w.next(), w.next()).unwrap();
        }
        b.push_str("</code></pre>");
        w.paragraph(&mut b);
    }
    b.push_str("</article>");
    page(
        "docs",
        "body{font-family:sans-serif;display:flex;margin:0}aside{width:240px;background:#f6f6f6;padding:8px}article{flex:1;padding:16px;max-width:760px}pre{background:#272822;color:#f8f8f2;padding:8px;font-family:monospace}",
        &b,
    )
}

fn rules() -> String {
    let mut w = Words(9);
    let mut css = String::from("body{font-family:sans-serif}");
    for i in 0..2000 {
        write!(
            css,
            ".c{i} > span.k{} {{color:hsl({},60%,40%);padding-left:{}px}}",
            i % 17,
            i % 360,
            i % 9
        )
        .unwrap();
        if i % 50 == 0 {
            write!(
                css,
                "@media (min-width:{}px){{.c{i}{{margin:{}px}}}}",
                600 + i % 900,
                i % 7
            )
            .unwrap();
        }
    }
    let mut b = String::new();
    for i in 0..600 {
        write!(
            b,
            "<div class=\"c{}\"><span class=\"k{}\">{} {}</span></div>",
            i * 3 % 2000,
            i % 17,
            w.next(),
            w.next()
        )
        .unwrap();
    }
    page("rules", &css, &b)
}

fn inline() -> String {
    let mut w = Words(10);
    let mut b = String::new();
    for _ in 0..60 {
        b.push_str("<p>");
        for i in 0..40 {
            let x = w.next();
            match i % 6 {
                0 => write!(b, "<b>{x}</b> ").unwrap(),
                1 => write!(b, "<code>{x}</code> ").unwrap(),
                2 => write!(b, "<span style=\"font-size:{}px\">{x}</span> ", 12 + i % 10).unwrap(),
                3 => write!(b, "<em>{x}</em> ").unwrap(),
                _ => write!(b, "{x} ").unwrap(),
            }
        }
        b.push_str("</p>");
    }
    page(
        "inline",
        "body{font-family:sans-serif;max-width:900px;margin:auto}code{background:#eee}",
        &b,
    )
}

fn deep() -> String {
    let mut b = String::new();
    for i in 0..200 {
        write!(
            b,
            "<div style=\"padding-left:2px;border-left:1px solid hsl({},50%,50%)\">{i}",
            i * 13 % 360
        )
        .unwrap();
    }
    for _ in 0..200 {
        b.push_str("</div>");
    }
    page("deep", "body{font-family:sans-serif;font-size:10px}", &b)
}
