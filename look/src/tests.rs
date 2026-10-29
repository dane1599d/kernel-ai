use super::*;
use std::format;
use std::path::Path;
use std::string::String;
use std::vec::Vec;

fn ratio(a: u32, b: u32) -> f64 {
    (contrast(a, b) * 100.0).round() / 100.0
}

#[test]
fn the_ratios_are_adr_0005s_table() {
    // Against `window`, as ADR 0005 §8 states them (dark / light).
    let rows: [(Token, f64, f64); 10] = [
        (Token::LineStrong, 3.30, 3.79),
        (Token::Text, 14.29, 15.90),
        (Token::Muted, 7.00, 6.93),
        (Token::Dim, 5.12, 5.77),
        (Token::Ai, 8.76, 5.42),
        (Token::Ask, 10.91, 6.00),
        (Token::Error, 6.40, 6.20),
        (Token::Ok, 9.97, 5.70),
        (Token::Accent, 8.81, 5.74),
        (Token::Focus, 14.29, 15.90),
    ];
    for (token, dark, light) in rows {
        assert_eq!(ratio(DARK.get(token), DARK.window), dark, "dark {token:?}");
        assert_eq!(
            ratio(LIGHT.get(token), LIGHT.window),
            light,
            "light {token:?}"
        );
    }
    assert_eq!(ratio(DARK.dim, DARK.raised), 4.70);
    assert_eq!(ratio(LIGHT.ai_mark, LIGHT.window), 4.23);
    assert_eq!(ratio(HIGH_CONTRAST.ai, HIGH_CONTRAST.window), 10.36);
}

/// The tokens that are text, held to 4.5:1 (WCAG 2.2 1.4.3).
const TEXT: [Token; 9] = [
    Token::Text,
    Token::Muted,
    Token::Dim,
    Token::Ai,
    Token::Ask,
    Token::Error,
    Token::Ok,
    Token::Accent,
    Token::Focus,
];
/// Marks and edges, held to 3:1 (WCAG 2.2 1.4.11). `line` is decorative.
const MARKS: [Token; 3] = [Token::LineStrong, Token::AiMark, Token::Focus];
/// What text and marks sit on.
const GROUNDS: [Token; 3] = [Token::Window, Token::Raised, Token::Desk];

#[test]
fn every_text_pair_is_at_least_4_5_and_every_mark_3() {
    for theme in THEMES {
        for ground in GROUNDS {
            for token in TEXT {
                let c = contrast(theme.get(token), theme.get(ground));
                assert!(c >= 4.5, "{} {token:?} on {ground:?}: {c:.2}", theme.name);
            }
            for token in MARKS {
                let c = contrast(theme.get(token), theme.get(ground));
                assert!(c >= 3.0, "{} {token:?} on {ground:?}: {c:.2}", theme.name);
            }
        }
    }
}

#[test]
fn the_console_keeps_its_contrast() {
    use console::*;
    for text in [PAPER, SLATE, DIM, EMBER, CORAL, GOLD, SAGE] {
        assert!(contrast(text, INK) >= 4.5, "{text:06x} on ink");
        assert!(contrast(text, BAR) >= 4.5, "{text:06x} on the status bar");
    }
    for (text, tint, bar) in [
        (REPLY_TEXT, REPLY_TINT, EMBER),
        (ERROR_TEXT, ERROR_TINT, CORAL),
        (QUESTION_TEXT, QUESTION_TINT, GOLD),
    ] {
        assert!(contrast(text, tint) >= 4.5, "{text:06x} on {tint:06x}");
        assert!(
            contrast(bar, tint) >= 3.0,
            "the bar {bar:06x} on {tint:06x}"
        );
    }
}

#[test]
fn apps_get_no_ai_or_ask_colour() {
    for t in [Token::Ai, Token::AiMark, Token::Ask] {
        assert!(!Token::APP.contains(&t), "{t:?}");
    }
    assert_eq!(Token::APP.len() + 3, Token::ALL.len());
}

#[test]
fn reduced_motion_is_instant_and_nothing_is_longer_than_half_a_second() {
    for m in [
        Motion::Instant,
        Motion::Quick,
        Motion::State,
        Motion::Sheet,
        Motion::Settle,
        Motion::Tick,
    ] {
        assert_eq!(motion(m, true), (0, Curve::None));
        assert!(motion(m, false).0 <= 500, "{m:?}");
    }
}

#[test]
fn legible_is_one_px_larger_in_atkinson() {
    for s in TypeStyle::ALL {
        let (a, b) = (style(s, false), style(s, true));
        assert_eq!(b.size, a.size + 1.0);
        assert!(matches!(
            b.family,
            Family::LegibleSans | Family::LegibleMono
        ));
    }
}

/// A colour written as a literal: `0x00RRGGBB` or `0xFFRRGGBB`, underscores
/// allowed, or `#RRGGBB` in a string.
fn literals(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'0' && bytes[i + 1] == b'x' {
            let hex: String = source[i + 2..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit() || *c == '_')
                .filter(|c| *c != '_')
                .collect();
            if hex.len() == 8
                && (hex.starts_with("00") || hex.to_ascii_uppercase().starts_with("FF"))
            {
                found.push(format!("0x{hex}"));
            }
            i += 2 + hex.len();
        } else if bytes[i] == b'#'
            && source[i + 1..]
                .chars()
                .take(6)
                .filter(char::is_ascii_hexdigit)
                .count()
                == 6
            && !source[i + 7..].starts_with(|c: char| c.is_ascii_alphanumeric())
        {
            found.push(source[i..i + 7].into());
            i += 7;
        } else {
            i += 1;
        }
    }
    found
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// ADR 0005 §8's lint: the toolkit, the compositor and the apps take colours
/// from tokens only. (The renderer `gfx2d` blends with channel masks, and the
/// kernel console is `console` here, so neither is scanned.)
#[test]
fn no_colour_literal_outside_look() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for dir in ["uikit", "deskcore", "components", "user/src/bin/desktop"] {
        rust_files(&root.join(dir), &mut files);
    }
    let one = root.join("user/src/bin/desktop.rs");
    if one.exists() {
        files.push(one);
    }
    let mut bad = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for lit in literals(&text) {
            bad.push(format!("{}: {lit}", file.display()));
        }
    }
    assert!(bad.is_empty(), "colour literals outside look: {bad:?}");
}

#[test]
fn the_lint_sees_literals_and_not_other_hex() {
    assert_eq!(literals("let c = 0x00FF_9F5A;"), ["0x00FF9F5A"]);
    assert_eq!(literals("x: 0xFF12_3456"), ["0xFF123456"]);
    assert_eq!(literals("\"#ECE6DA\""), ["#ECE6DA"]);
    assert!(literals("0x1000 0x2000_0000_0000 #[test] #123 0xFF").is_empty());
}
