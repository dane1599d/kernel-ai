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

/// ADR 0005 §8's colour table is the palette: every token's value in the dark,
/// light and high-contrast themes, read back from the document, so the two
/// cannot drift apart.
#[test]
fn the_adr_colour_table_is_the_palette() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/adr/0005-graphics.md");
    let text = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let hex = |cell: &str| -> Option<u32> {
        let digits = cell.strip_prefix('`')?.strip_suffix('`')?;
        (digits.len() == 6)
            .then(|| u32::from_str_radix(digits, 16).ok())
            .flatten()
    };
    let mut seen: Vec<Token> = Vec::new();
    for line in text.lines() {
        // `| token | dark | light | high contrast | job |`
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 6 {
            continue;
        }
        let Some(at) = theme::TOKEN_NAMES.iter().position(|n| *n == cells[1]) else {
            continue;
        };
        let (Some(dark), Some(light), Some(high)) = (hex(cells[2]), hex(cells[3]), hex(cells[4]))
        else {
            continue;
        };
        let token = Token::ALL[at];
        assert_eq!(dark, DARK.get(token), "dark {token:?}");
        assert_eq!(light, LIGHT.get(token), "light {token:?}");
        assert_eq!(high, HIGH_CONTRAST.get(token), "high-contrast {token:?}");
        assert!(!seen.contains(&token), "{token:?} is in the table twice");
        seen.push(token);
    }
    for token in Token::ALL {
        assert!(seen.contains(&token), "{token:?} is not in the ADR's table");
    }
    assert_eq!(seen.len(), 17);
}

/// The ratios ADR 0005 §8's "Kai Desk" amendment states for the two new tokens.
#[test]
fn the_ratios_of_shell_and_hover_are_the_amendments() {
    // `shell` is a full ground: its worst text and mark, per theme.
    assert_eq!(ratio(DARK.dim, DARK.shell), 5.26);
    assert_eq!(ratio(DARK.line_strong, DARK.shell), 3.39);
    assert_eq!(ratio(LIGHT.ai, LIGHT.shell), 5.06);
    assert_eq!(ratio(LIGHT.line_strong, LIGHT.shell), 3.54);
    // `hover` is a restricted ground: its worst text and mark.
    assert_eq!(ratio(DARK.error, DARK.hover), 5.12);
    assert_eq!(ratio(LIGHT.ok, LIGHT.hover), 4.56);
    assert_eq!(ratio(LIGHT.accent, LIGHT.hover), 4.59);
    assert_eq!(ratio(LIGHT.ai_mark, LIGHT.hover), 3.38);
    // The step a hover fill makes against its ground.
    assert_eq!(ratio(DARK.hover, DARK.shell), 1.28);
    assert_eq!(ratio(LIGHT.hover, LIGHT.shell), 1.17);
    // Why the Kai mark is `ai_mark` while hovered: `ai` is under 4.5:1 on it.
    assert_eq!(ratio(LIGHT.ai, LIGHT.hover), 4.33);
}

/// Desk, shell, window and raised are layered, each lighter than the last in
/// both themes, so a taskbar reads as sitting above the desk and a menu above a
/// window.
#[test]
fn the_grounds_are_layered_desk_shell_window_raised() {
    for theme in [DARK, LIGHT] {
        let l = |t| luminance(theme.get(t));
        assert!(l(Token::Desk) < l(Token::Shell), "{}", theme.name);
        assert!(l(Token::Shell) < l(Token::Window), "{}", theme.name);
        assert!(l(Token::Window) < l(Token::Raised), "{}", theme.name);
    }
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
/// What text and marks sit on, all four held to the full gate. `shell` joined
/// them with the "Kai Desk" amendment.
const GROUNDS: [Token; 4] = [Token::Window, Token::Raised, Token::Desk, Token::Shell];
/// `hover` is a restricted ground. Text drawn on it, held to 4.5:1 there.
const HOVER_TEXT: [Token; 5] = [
    Token::Text,
    Token::Muted,
    Token::Accent,
    Token::Error,
    Token::Ok,
];
/// Marks drawn on it, held to 3:1 there.
const HOVER_MARKS: [Token; 2] = [Token::Focus, Token::AiMark];
/// Never drawn on it.
const HOVER_NEVER: [Token; 3] = [Token::Dim, Token::Ai, Token::LineStrong];

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
fn what_is_drawn_on_hover_holds_its_contrast() {
    for theme in THEMES {
        for token in HOVER_TEXT {
            let c = contrast(theme.get(token), theme.hover);
            assert!(c >= 4.5, "{} {token:?} on hover: {c:.2}", theme.name);
        }
        for token in HOVER_MARKS {
            let c = contrast(theme.get(token), theme.hover);
            assert!(c >= 3.0, "{} {token:?} on hover: {c:.2}", theme.name);
        }
    }
}

/// `hover` is restricted because three tokens fall short on it in the dark or the
/// light theme: that is why they are never drawn there, and the gate's lists in
/// `theme` say the same as this file's.
#[test]
fn what_is_never_drawn_on_hover_falls_short_of_its_gate_there() {
    assert_eq!(theme::GROUNDS, GROUNDS);
    assert_eq!(theme::HOVER_TEXT, HOVER_TEXT);
    assert_eq!(theme::HOVER_MARKS, HOVER_MARKS);
    assert_eq!(theme::HOVER_NEVER, HOVER_NEVER);
    let short = |token: Token, least: f64| {
        THEMES
            .iter()
            .any(|t| contrast(t.get(token), t.hover) < least)
    };
    assert!(short(Token::Dim, 4.5));
    assert!(short(Token::Ai, 4.5));
    assert!(short(Token::LineStrong, 3.0));
    for token in HOVER_NEVER {
        assert!(!HOVER_TEXT.contains(&token) && !HOVER_MARKS.contains(&token));
    }
}

/// High contrast has one ground colour. `hover` equals it, which is the cue a
/// painter reads to draw a 1 px `focus` frame instead of a fill.
#[test]
fn high_contrast_hover_is_its_ground() {
    let hc = HIGH_CONTRAST;
    for ground in [hc.desk, hc.shell, hc.window, hc.raised] {
        assert_eq!(hc.hover, ground);
    }
    assert_ne!(hc.hover, hc.focus);
    // The other themes have a visible fill.
    for theme in [DARK, LIGHT] {
        assert_ne!(theme.hover, theme.shell, "{}", theme.name);
        assert_ne!(theme.hover, theme.window, "{}", theme.name);
        assert_ne!(theme.hover, theme.raised, "{}", theme.name);
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
    // The AI's and the trusted layer's tokens, and the desktop's own two: an app
    // draws with none of them.
    let kept_back = [
        Token::Ai,
        Token::AiMark,
        Token::Ask,
        Token::Shell,
        Token::Hover,
    ];
    for t in kept_back {
        assert!(!Token::APP.contains(&t), "{t:?}");
    }
    assert_eq!(Token::APP.len() + kept_back.len(), Token::ALL.len());
    assert_eq!(Token::ALL.len(), 17);
    assert_eq!(Token::APP.len(), 12);
}

#[test]
fn every_token_has_one_name_and_one_place() {
    for (at, token) in Token::ALL.iter().enumerate() {
        assert!(!Token::ALL[..at].contains(token), "{token:?} twice");
    }
    for palette in THEMES {
        // `get` answers every token, in the palette's own fields.
        let values = Token::ALL.map(|t| palette.get(t));
        assert_eq!(values.len(), theme::TOKEN_NAMES.len(), "{}", palette.name);
    }
}

/// The type scale ADR 0005 §8 names: the seven styles, unchanged by "Kai Desk"
/// (no size is added, so the glyph atlas gains no key).
#[test]
fn the_type_scale_is_adr_0005s_table() {
    let rows: [(TypeStyle, Family, f32, f32, u16); 7] = [
        (TypeStyle::Small, Family::Sans, 12.0, 16.0, 400),
        (TypeStyle::Body, Family::Sans, 14.0, 20.0, 400),
        (TypeStyle::Label, Family::Sans, 13.0, 18.0, 500),
        (TypeStyle::Heading, Family::Sans, 18.0, 24.0, 600),
        (TypeStyle::Display, Family::Sans, 28.0, 34.0, 600),
        (TypeStyle::Mono, Family::Mono, 14.0, 20.0, 400),
        (TypeStyle::MonoTitle, Family::Mono, 12.0, 16.0, 500),
    ];
    for (s, family, size, line, weight) in rows {
        let t = style(s, false);
        assert_eq!(
            (t.family, t.size, t.line, t.weight),
            (family, size, line, weight)
        );
    }
    assert_eq!(TypeStyle::ALL.len(), 7);
}

/// The metrics of ADR 0005 §8: one 4 px grid, and the chrome's numbers sit on it
/// (the three that do not say why in `metric`).
#[test]
fn the_metrics_sit_on_the_4_px_grid() {
    use metric::*;
    for n in SPACE {
        assert_eq!(n % GRID, 0, "{n}");
    }
    assert!(SPACE.windows(2).all(|w| w[0] < w[1]));
    for n in [
        TASKBAR,
        ITEM,
        ITEM_GAP,
        ICON_BAR,
        ICON_TILE,
        ICON_TITLE,
        FLYOUT_MARGIN,
        TITLE_BAR,
        CONTROL_W,
        CONTROL_H,
        SHADOW_REACH,
        GRAB_CORNER,
        RADIUS_SM,
        RADIUS_MD,
    ] {
        assert_eq!(n % GRID, 0, "{n}");
    }
    // The off-grid three: a hit area, a frame and a radius that nests.
    assert_eq!((GRAB, FRAME, RADIUS_FIELD), (6, 1, 6));
}

/// How the metrics fit one another is checked as the crate builds
/// (`metric`'s compile-time assertions); these are the two functions.
#[test]
fn a_pill_is_half_its_height_and_a_maximized_window_is_square() {
    use metric::*;
    assert_eq!((pill(40), pill(32), pill(28)), (20, 16, 14));
    assert_eq!((window_radius(false), window_radius(true)), (RADIUS_MD, 0));
    assert_eq!(
        (RADIUS_SM, RADIUS_FIELD, RADIUS_MD, SHADOW_REACH, GRAB),
        (4, 6, 8, 12, 6)
    );
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
