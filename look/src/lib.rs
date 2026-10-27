//! "Ember Workbench", ADR 0005 §8: the one source of Kernel AI's colours, type
//! styles and motion values.
//!
//! - [`Token`] and [`Palette`]: the colour tokens, each with one job, in the dark,
//!   light and high-contrast themes. `ai` is the AI's colour and nothing else's;
//!   apps never get it (ADR 0005 §4), and `ask` belongs to `desktop`'s trusted
//!   layer.
//! - [`console`]: the kernel console's row tints, #17's terminal colours, which
//!   stay the console's alone.
//! - [`TypeStyle`] and [`Type`]: the type styles in logical px.
//! - [`Motion`]: the motion tokens.
//! - [`contrast`]: the WCAG 2.2 contrast ratio. The tests hold every text pair
//!   to 4.5:1 and every mark and edge to 3:1, and fail any colour literal outside
//!   this crate and the kernel console.
//!
//! Colours are `0x00RRGGBB`.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod console;

/// A colour's job (ADR 0005 §8's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    /// The desktop.
    Desk,
    /// Windows, the top bar and the command line.
    Window,
    /// Sheets and menus.
    Raised,
    /// Dividers only: decorative, held to no contrast.
    Line,
    /// Window frames and inputs.
    LineStrong,
    Text,
    Muted,
    Dim,
    /// The AI and nothing else: its text.
    Ai,
    /// The AI's marks (the outline, the caret, the listening rule). The same as
    /// `ai` in the dark theme; a stronger shade in the light one.
    AiMark,
    /// Waiting for you.
    Ask,
    /// Failed or refused.
    Error,
    /// Done, undo available.
    Ok,
    /// Apps and links.
    Accent,
    /// A 2 px ring with a 1 px gap.
    Focus,
}

impl Token {
    pub const ALL: [Token; 15] = [
        Token::Desk,
        Token::Window,
        Token::Raised,
        Token::Line,
        Token::LineStrong,
        Token::Text,
        Token::Muted,
        Token::Dim,
        Token::Ai,
        Token::AiMark,
        Token::Ask,
        Token::Error,
        Token::Ok,
        Token::Accent,
        Token::Focus,
    ];

    /// The tokens an app may draw with: none of the AI's or the trusted layer's.
    pub const APP: [Token; 12] = [
        Token::Desk,
        Token::Window,
        Token::Raised,
        Token::Line,
        Token::LineStrong,
        Token::Text,
        Token::Muted,
        Token::Dim,
        Token::Error,
        Token::Ok,
        Token::Accent,
        Token::Focus,
    ];
}

/// A theme: a colour for every token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub name: &'static str,
    pub dark: bool,
    pub desk: u32,
    pub window: u32,
    pub raised: u32,
    pub line: u32,
    pub line_strong: u32,
    pub text: u32,
    pub muted: u32,
    pub dim: u32,
    pub ai: u32,
    pub ai_mark: u32,
    pub ask: u32,
    pub error: u32,
    pub ok: u32,
    pub accent: u32,
    pub focus: u32,
}

impl Palette {
    pub const fn get(&self, token: Token) -> u32 {
        match token {
            Token::Desk => self.desk,
            Token::Window => self.window,
            Token::Raised => self.raised,
            Token::Line => self.line,
            Token::LineStrong => self.line_strong,
            Token::Text => self.text,
            Token::Muted => self.muted,
            Token::Dim => self.dim,
            Token::Ai => self.ai,
            Token::AiMark => self.ai_mark,
            Token::Ask => self.ask,
            Token::Error => self.error,
            Token::Ok => self.ok,
            Token::Accent => self.accent,
            Token::Focus => self.focus,
        }
    }
}

/// Dark first: #17's palette (the kernel console's), plus `raised`, `line_strong`
/// and `accent`.
pub const DARK: Palette = Palette {
    name: "dark",
    dark: true,
    desk: 0x0D_1017,
    window: 0x14_1821,
    raised: 0x1B_202B,
    line: 0x23_2A38,
    line_strong: 0x5F_6B80,
    text: 0xEC_E6DA,
    muted: 0x9A_A3B4,
    dim: 0x7C_8AA8,
    ai: 0xFF_9F5A,
    ai_mark: 0xFF_9F5A,
    ask: 0xF5_C451,
    error: 0xFF_6B6B,
    ok: 0x7F_D49B,
    accent: 0x8C_B8FF,
    focus: 0xEC_E6DA,
};

pub const LIGHT: Palette = Palette {
    name: "light",
    dark: false,
    desk: 0xE9_E4DA,
    window: 0xFA_F8F4,
    raised: 0xFF_FFFF,
    line: 0xD9_D3C7,
    line_strong: 0x85_7E71,
    text: 0x1A_1D24,
    muted: 0x4F_5666,
    dim: 0x5B_6271,
    ai: 0xA8_4A0C,
    ai_mark: 0xC4_560C,
    ask: 0x7F_5800,
    error: 0xB4_2318,
    ok: 0x26_703F,
    accent: 0x1F_5FBF,
    focus: 0x1A_1D24,
};

/// Black and white, with ember kept for the AI (10.36:1 on black).
pub const HIGH_CONTRAST: Palette = Palette {
    name: "high-contrast",
    dark: true,
    desk: 0x00_0000,
    window: 0x00_0000,
    raised: 0x00_0000,
    line: 0xFF_FFFF,
    line_strong: 0xFF_FFFF,
    text: 0xFF_FFFF,
    muted: 0xFF_FFFF,
    dim: 0xFF_FFFF,
    ai: 0xFF_9F5A,
    ai_mark: 0xFF_9F5A,
    ask: 0xFF_FFFF,
    error: 0xFF_FFFF,
    ok: 0xFF_FFFF,
    accent: 0xFF_FFFF,
    focus: 0xFF_FFFF,
};

pub const THEMES: [Palette; 3] = [DARK, LIGHT, HIGH_CONTRAST];

/// A family ADR 0005 §8 names; all three are SIL OFL-1.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// Inter 4.1.
    Sans,
    /// JetBrains Mono 2.304.
    Mono,
    /// Atkinson Hyperlegible Next, for the legible setting.
    LegibleSans,
    /// Atkinson Hyperlegible Mono, for the legible setting.
    LegibleMono,
}

/// What a type style is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeStyle {
    /// Captions.
    Small,
    /// Text.
    Body,
    /// Buttons and menus.
    Label,
    Heading,
    /// Settings titles.
    Display,
    /// Terminal, editor, code in asks.
    Mono,
    /// Window titles, tabs, `kai>`, action lines.
    MonoTitle,
}

impl TypeStyle {
    pub const ALL: [TypeStyle; 7] = [
        TypeStyle::Small,
        TypeStyle::Body,
        TypeStyle::Label,
        TypeStyle::Heading,
        TypeStyle::Display,
        TypeStyle::Mono,
        TypeStyle::MonoTitle,
    ];
}

/// A style's family, size and line height in logical px, weight and optical size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Type {
    pub family: Family,
    pub size: f32,
    pub line: f32,
    pub weight: u16,
    /// The `opsz` axis, where the style sets one.
    pub opsz: Option<f32>,
}

/// `style`'s type; with `legible`, Atkinson Hyperlegible one px larger.
pub fn style(style: TypeStyle, legible: bool) -> Type {
    let (family, size, line, weight, opsz) = match style {
        TypeStyle::Small => (Family::Sans, 12.0, 16.0, 400, None),
        TypeStyle::Body => (Family::Sans, 14.0, 20.0, 400, None),
        TypeStyle::Label => (Family::Sans, 13.0, 18.0, 500, None),
        TypeStyle::Heading => (Family::Sans, 18.0, 24.0, 600, None),
        TypeStyle::Display => (Family::Sans, 28.0, 34.0, 600, Some(32.0)),
        TypeStyle::Mono => (Family::Mono, 14.0, 20.0, 400, None),
        TypeStyle::MonoTitle => (Family::Mono, 12.0, 16.0, 500, None),
    };
    match legible {
        false => Type {
            family,
            size,
            line,
            weight,
            opsz,
        },
        true => Type {
            family: match family {
                Family::Mono => Family::LegibleMono,
                _ => Family::LegibleSans,
            },
            size: size + 1.0,
            line: line + 1.0,
            weight,
            opsz: None,
        },
    }
}

/// How a motion token moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    None,
    /// A cubic Bézier's two control points.
    Bezier(f32, f32, f32, f32),
    Linear,
    /// A spring: stiffness and damping, mass 1; translate only, interruptible.
    Spring {
        stiffness: f32,
        damping: f32,
    },
    /// Discrete steps.
    Steps,
}

/// The motion tokens. Nothing the user starts waits behind an animation; only
/// AI surfaces move, and nothing runs longer than 0.5 s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Motion {
    /// Typing, the caret, focus; windows opening, closing and raising; menus.
    Instant,
    /// Hover and press on controls (opacity).
    Quick,
    /// The command line's state changes (cross-fade).
    State,
    /// The sheet.
    Sheet,
    /// The agent's outline after an action (opacity).
    Settle,
    /// The thinking dots.
    Tick,
}

/// `motion`'s duration in ms and curve; with `reduced` (the setting, and every
/// `test=` boot) all are instant.
pub fn motion(motion: Motion, reduced: bool) -> (u32, Curve) {
    if reduced {
        return (0, Curve::None);
    }
    match motion {
        Motion::Instant => (0, Curve::None),
        Motion::Quick => (90, Curve::Bezier(0.2, 0.0, 0.0, 1.0)),
        Motion::State => (160, Curve::Linear),
        Motion::Sheet => (
            280,
            Curve::Spring {
                stiffness: 503.6,
                damping: 44.9,
            },
        ),
        Motion::Settle => (480, Curve::Linear),
        Motion::Tick => (125, Curve::Steps),
    }
}

/// A channel's linear light, per sRGB.
fn linear(c: u32) -> f64 {
    let c = f64::from(c & 0xFF) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        libm::pow((c + 0.055) / 1.055, 2.4)
    }
}

/// WCAG 2.2's relative luminance of `0x00RRGGBB`.
pub fn luminance(rgb: u32) -> f64 {
    0.2126 * linear(rgb >> 16) + 0.7152 * linear(rgb >> 8) + 0.0722 * linear(rgb)
}

/// WCAG 2.2's contrast ratio of two colours, 1 to 21.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests;
