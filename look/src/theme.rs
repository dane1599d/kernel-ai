//! `kai-theme 1` (ADR 0005 §8): a theme as a text file the user may edit.
//!
//! ```text
//! kai-theme 1
//! name = legible
//! base = light
//! text = #1A1D24
//! body = atkinson-next 15/21 400
//! sheet = 280 spring 503.6 44.9
//! ```
//!
//! - The first line is `kai-theme 1`. Then `name` and `base`, in that order:
//!   the theme's name (lower-case letters, digits and `-`, at most
//!   [`MAX_NAME`]) and the built-in theme it starts from (`dark`, `light`,
//!   `high-contrast`).
//! - Then any of: a colour token (`text = #RRGGBB`, [`TOKEN_NAMES`]), a type
//!   style (`body = family size/line weight`, then `opsz n` where the style
//!   sets an optical size; [`STYLE_NAMES`], families
//!   [`FAMILY_NAMES`]) or a motion (`sheet = ms curve`, [`MOTION_NAMES`]; curves
//!   `none`, `linear`, `steps`, `bezier x1 y1 x2 y2`, `spring stiffness
//!   damping`), each at most once. Blank lines and lines starting with `;` are
//!   skipped.
//! - What the file leaves out is the base's. A theme that parses must still
//!   pass [`Theme::check`]: every text token at least 4.5:1 and every mark 3:1
//!   on every ground (WCAG 2.2), the AI's colour unlike any colour an app may
//!   draw with (ADR 0005 §4), sizes and motions in range.
//!
//! [`Theme::encode`] writes every value, so the shipped themes are whole files.

use crate::{Curve, DARK, Family, HIGH_CONTRAST, LIGHT, Motion, Palette, Token, Type, TypeStyle};
use alloc::format;
use alloc::string::String;
use core::fmt::Write;

/// The first line.
pub const MAGIC: &str = "kai-theme 1";
/// The longest file read.
pub const MAX_BYTES: usize = 8 * 1024;
/// The longest theme name.
pub const MAX_NAME: usize = 32;
/// The longest motion a theme may set (ADR 0005 §8: nothing runs past 0.5 s).
pub const MAX_MOTION_MS: u32 = 500;

/// The file's names for [`Token::ALL`], in its order.
pub const TOKEN_NAMES: [&str; 15] = [
    "desk",
    "window",
    "raised",
    "line",
    "line_strong",
    "text",
    "muted",
    "dim",
    "ai",
    "ai_mark",
    "ask",
    "error",
    "ok",
    "accent",
    "focus",
];
/// The file's names for [`TypeStyle::ALL`], in its order.
pub const STYLE_NAMES: [&str; 7] = [
    "small",
    "body",
    "label",
    "heading",
    "display",
    "mono",
    "mono_title",
];
/// The file's names for [`MOTIONS`], in its order.
pub const MOTION_NAMES: [&str; 6] = ["instant", "quick", "state", "sheet", "settle", "tick"];
/// Every motion token, in the file's order.
pub const MOTIONS: [Motion; 6] = [
    Motion::Instant,
    Motion::Quick,
    Motion::State,
    Motion::Sheet,
    Motion::Settle,
    Motion::Tick,
];
/// The file's names for the families.
pub const FAMILY_NAMES: [(&str, Family); 4] = [
    ("inter", Family::Sans),
    ("jetbrains-mono", Family::Mono),
    ("atkinson-next", Family::LegibleSans),
    ("atkinson-mono", Family::LegibleMono),
];
/// Text tokens, held to 4.5:1 (WCAG 2.2 1.4.3).
pub const TEXT: [Token; 9] = [
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
pub const MARKS: [Token; 3] = [Token::LineStrong, Token::AiMark, Token::Focus];
/// What text and marks sit on.
pub const GROUNDS: [Token; 3] = [Token::Window, Token::Raised, Token::Desk];

/// A whole theme: a colour for every token, a type for every style and a
/// motion for every motion token.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: String,
    /// The built-in theme it starts from; it sets whether the theme is dark.
    pub base: &'static Palette,
    colours: [u32; 15],
    styles: [Type; 7],
    motions: [(u32, Curve); 6],
}

/// Why a file is not a theme: its line (1 for the first) and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeError {
    pub line: usize,
    pub why: String,
}

fn error(line: usize, why: impl Into<String>) -> ThemeError {
    ThemeError {
        line,
        why: why.into(),
    }
}

/// The built-in themes a file may start from.
pub fn base(name: &str) -> Option<&'static Palette> {
    [&DARK, &LIGHT, &HIGH_CONTRAST]
        .into_iter()
        .find(|palette| palette.name == name)
}

fn position<T: PartialEq>(all: &[T], item: &T) -> usize {
    all.iter().position(|x| x == item).unwrap_or(0)
}

impl Theme {
    /// The theme a built-in palette is, with the default type and motion.
    pub fn from_palette(palette: &'static Palette, legible: bool) -> Theme {
        Theme {
            name: String::from(palette.name),
            base: palette,
            colours: Token::ALL.map(|token| palette.get(token)),
            styles: TypeStyle::ALL.map(|style| crate::style(style, legible)),
            motions: MOTIONS.map(|motion| crate::motion(motion, false)),
        }
    }

    pub fn dark(&self) -> bool {
        self.base.dark
    }

    /// The colours as a [`Palette`], named after the base, for painters that
    /// take one.
    pub fn palette(&self) -> Palette {
        let c = |token| self.colour(token);
        Palette {
            name: self.base.name,
            dark: self.base.dark,
            desk: c(Token::Desk),
            window: c(Token::Window),
            raised: c(Token::Raised),
            line: c(Token::Line),
            line_strong: c(Token::LineStrong),
            text: c(Token::Text),
            muted: c(Token::Muted),
            dim: c(Token::Dim),
            ai: c(Token::Ai),
            ai_mark: c(Token::AiMark),
            ask: c(Token::Ask),
            error: c(Token::Error),
            ok: c(Token::Ok),
            accent: c(Token::Accent),
            focus: c(Token::Focus),
        }
    }

    pub fn colour(&self, token: Token) -> u32 {
        self.colours[position(&Token::ALL, &token)]
    }

    pub fn style(&self, style: TypeStyle) -> Type {
        self.styles[position(&TypeStyle::ALL, &style)]
    }

    /// `motion`'s duration in ms and curve; with `reduced`, all are instant.
    pub fn motion(&self, motion: Motion, reduced: bool) -> (u32, Curve) {
        match reduced {
            true => (0, Curve::None),
            false => self.motions[position(&MOTIONS, &motion)],
        }
    }

    /// The same theme with `token` in `rgb`, unchecked: [`Theme::check`] it.
    pub fn with_colour(&self, token: Token, rgb: u32) -> Theme {
        let mut theme = self.clone();
        theme.colours[position(&Token::ALL, &token)] = rgb & 0xFF_FFFF;
        theme
    }

    /// The same theme in Atkinson Hyperlegible, 1 px larger (the legible
    /// setting), and text `percent` of its size (100 to 200).
    pub fn sized(&self, legible: bool, percent: u16) -> Theme {
        let mut theme = self.clone();
        let factor = f32::from(percent) / 100.0;
        for (at, style) in TypeStyle::ALL.iter().enumerate() {
            let mut ty = self.styles[at];
            if legible && !matches!(ty.family, Family::LegibleSans | Family::LegibleMono) {
                let legible = crate::style(*style, true);
                ty.family = legible.family;
                ty.size += 1.0;
                ty.line += 1.0;
                ty.opsz = None;
            }
            ty.size = libm::roundf(ty.size * factor * 4.0) / 4.0;
            ty.line = libm::roundf(ty.line * factor * 4.0) / 4.0;
            theme.styles[at] = ty;
        }
        theme
    }

    /// Reads a `kai-theme 1` file and checks it.
    pub fn parse(text: &str) -> Result<Theme, ThemeError> {
        if text.len() > MAX_BYTES {
            return Err(error(0, format!("longer than {MAX_BYTES} bytes")));
        }
        let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
        if lines.next() != Some(MAGIC) {
            return Err(error(1, format!("the first line is not {MAGIC:?}")));
        }
        let mut name = None;
        let mut theme: Option<Theme> = None;
        let mut seen = [false; 15 + 7 + 6];
        for (at, line) in lines.enumerate() {
            let n = at + 2;
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            let (key, value) = line
                .split_once(" = ")
                .ok_or_else(|| error(n, "not `key = value`"))?;
            const FIRST: &str = "`name` and then `base` come first, once each";
            match key {
                "name" if name.is_none() => {
                    name = Some(parse_name(value).ok_or_else(|| error(n, "a bad name"))?);
                }
                "base" if name.is_some() && theme.is_none() => {
                    let palette = base(value).ok_or_else(|| error(n, "no such base theme"))?;
                    let mut start = Theme::from_palette(palette, false);
                    start.name = name.clone().unwrap_or_default();
                    theme = Some(start);
                }
                "name" | "base" => return Err(error(n, FIRST)),
                key => {
                    let theme = theme.as_mut().ok_or_else(|| error(n, FIRST))?;
                    let slot = theme.set(key, value).map_err(|why| error(n, why))?;
                    if core::mem::replace(&mut seen[slot], true) {
                        return Err(error(n, format!("{key} is given twice")));
                    }
                }
            }
        }
        let theme = theme.ok_or_else(|| error(0, "no `name` and `base`"))?;
        theme.check().map_err(|why| error(0, why))?;
        Ok(theme)
    }

    /// Sets `key` to `value`; returns the key's slot, for repeats.
    fn set(&mut self, key: &str, value: &str) -> Result<usize, &'static str> {
        if let Some(at) = TOKEN_NAMES.iter().position(|n| *n == key) {
            self.colours[at] = parse_colour(value).ok_or("a colour is `#RRGGBB`")?;
            return Ok(at);
        }
        if let Some(at) = STYLE_NAMES.iter().position(|n| *n == key) {
            self.styles[at] = parse_type(value).ok_or("a style is `family size/line weight`")?;
            return Ok(15 + at);
        }
        if let Some(at) = MOTION_NAMES.iter().position(|n| *n == key) {
            self.motions[at] = parse_motion(value).ok_or("a motion is `ms curve`")?;
            return Ok(15 + 7 + at);
        }
        Err("no such token, style or motion")
    }

    /// Whether the theme may be shown: contrast, the AI's colour, sizes and
    /// motions.
    pub fn check(&self) -> Result<(), String> {
        for ground in GROUNDS {
            let on = self.colour(ground);
            for (tokens, least) in [(&TEXT[..], 4.5), (&MARKS[..], 3.0)] {
                for &token in tokens {
                    let ratio = crate::contrast(self.colour(token), on);
                    if ratio < least {
                        return Err(format!(
                            "{} on {} is {ratio:.2}:1, under {least}:1",
                            TOKEN_NAMES[position(&Token::ALL, &token)],
                            TOKEN_NAMES[position(&Token::ALL, &ground)],
                        ));
                    }
                }
            }
        }
        let ai = self.colour(Token::Ai);
        if Token::APP.iter().any(|&token| self.colour(token) == ai) {
            return Err(String::from("ai's colour is one an app may draw with"));
        }
        for ty in &self.styles {
            if !type_in_range(ty) {
                return Err(String::from("a type style is out of range"));
            }
        }
        for (ms, curve) in &self.motions {
            if *ms > MAX_MOTION_MS || !curve_in_range(curve) {
                return Err(String::from("a motion is out of range"));
            }
        }
        Ok(())
    }

    /// The whole theme as a `kai-theme 1` file.
    pub fn encode(&self) -> String {
        let mut out = format!("{MAGIC}\nname = {}\nbase = {}\n", self.name, self.base.name);
        for (name, rgb) in TOKEN_NAMES.iter().zip(self.colours) {
            let _ = writeln!(out, "{name} = #{rgb:06X}");
        }
        for (name, ty) in STYLE_NAMES.iter().zip(self.styles) {
            let family = FAMILY_NAMES
                .iter()
                .find(|(_, f)| *f == ty.family)
                .map_or("inter", |(n, _)| n);
            let _ = write!(
                out,
                "{name} = {family} {}/{} {}",
                ty.size, ty.line, ty.weight
            );
            let _ = match ty.opsz {
                Some(opsz) => writeln!(out, " opsz {opsz}"),
                None => writeln!(out),
            };
        }
        for (name, (ms, curve)) in MOTION_NAMES.iter().zip(self.motions) {
            let _ = match curve {
                Curve::None => writeln!(out, "{name} = {ms} none"),
                Curve::Linear => writeln!(out, "{name} = {ms} linear"),
                Curve::Steps => writeln!(out, "{name} = {ms} steps"),
                Curve::Bezier(a, b, c, d) => writeln!(out, "{name} = {ms} bezier {a} {b} {c} {d}"),
                Curve::Spring { stiffness, damping } => {
                    writeln!(out, "{name} = {ms} spring {stiffness} {damping}")
                }
            };
        }
        out
    }
}

fn parse_name(value: &str) -> Option<String> {
    let ok = !value.is_empty()
        && value.len() <= MAX_NAME
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| String::from(value))
}

/// `#RRGGBB`.
pub fn parse_colour(value: &str) -> Option<u32> {
    let hex = value.strip_prefix('#')?;
    (hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
}

fn number(word: &str) -> Option<f32> {
    word.parse::<f32>().ok().filter(|n| n.is_finite())
}

fn parse_type(value: &str) -> Option<Type> {
    let mut words = value.split(' ');
    let family = words.next()?;
    let (size, line) = words.next()?.split_once('/')?;
    let weight = words.next()?.parse::<u16>().ok()?;
    let opsz = match words.next() {
        None => None,
        Some("opsz") => Some(number(words.next()?).filter(|n| (6.0..=144.0).contains(n))?),
        Some(_) => return None,
    };
    if words.next().is_some() {
        return None;
    }
    let family = FAMILY_NAMES.iter().find(|(n, _)| *n == family)?.1;
    let ty = Type {
        family,
        size: number(size)?,
        line: number(line)?,
        weight,
        opsz,
    };
    type_in_range(&ty).then_some(ty)
}

fn type_in_range(ty: &Type) -> bool {
    (8.0..=96.0).contains(&ty.size)
        && ty.line >= ty.size
        && ty.line <= ty.size * 3.0
        && (100..=900).contains(&ty.weight)
}

fn parse_motion(value: &str) -> Option<(u32, Curve)> {
    let mut words = value.split(' ');
    let ms = words.next()?.parse::<u32>().ok()?;
    let mut numbers = [0.0f32; 4];
    let curve = match words.next()? {
        "none" => Curve::None,
        "linear" => Curve::Linear,
        "steps" => Curve::Steps,
        kind @ ("bezier" | "spring") => {
            let count = if kind == "bezier" { 4 } else { 2 };
            for slot in numbers.iter_mut().take(count) {
                *slot = number(words.next()?)?;
            }
            match kind {
                "bezier" => Curve::Bezier(numbers[0], numbers[1], numbers[2], numbers[3]),
                _ => Curve::Spring {
                    stiffness: numbers[0],
                    damping: numbers[1],
                },
            }
        }
        _ => return None,
    };
    let fits = words.next().is_none() && ms <= MAX_MOTION_MS && curve_in_range(&curve);
    fits.then_some((ms, curve))
}

fn curve_in_range(curve: &Curve) -> bool {
    match *curve {
        Curve::Bezier(x1, y1, x2, y2) => {
            (0.0..=1.0).contains(&x1)
                && (0.0..=1.0).contains(&x2)
                && (-2.0..=3.0).contains(&y1)
                && (-2.0..=3.0).contains(&y2)
        }
        Curve::Spring { stiffness, damping } => {
            stiffness > 0.0 && stiffness <= 5000.0 && damping > 0.0 && damping <= 500.0
        }
        Curve::None | Curve::Linear | Curve::Steps => true,
    }
}

/// The themes Kernel AI ships, as files: dark, light, high-contrast and the
/// legible one (light, in Atkinson Hyperlegible).
pub fn shipped() -> [Theme; 4] {
    let mut legible = Theme::from_palette(&LIGHT, true);
    legible.name = String::from("legible");
    [
        Theme::from_palette(&DARK, false),
        Theme::from_palette(&LIGHT, false),
        Theme::from_palette(&HIGH_CONTRAST, false),
        legible,
    ]
}
