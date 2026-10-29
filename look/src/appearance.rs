//! Appearance (ADR 0005 §8): what `appearance_set(setting, value)` changes,
//! one run-tier row with undo. A setting gives a new [`Appearance`] and leaves
//! the old one as it was, so `undo` is putting the old one back.
//!
//! - `theme`: a theme's name ([`Themes`] says which exist).
//! - `token`: one colour over the theme, `name #RRGGBB` (`text #F0E8DC`), or
//!   `name` alone to drop it; an app-only colour cannot become the AI's.
//! - `scale`: the screen's scale in percent, 100 to 300 in steps of 25.
//! - `text_size`: text in percent of its size, 100 to 200 in steps of 10.
//! - `reduced_motion`, `legible`: `on` or `off`.
//!
//! Every setting is checked against the theme it ends in: a token or a theme
//! that would break [`Theme::check`] (contrast, the AI's colour) is refused.
//! Saved as a `kai-appearance 1` file, one `setting = value` a line.

use crate::Token;
use crate::theme::{self, TOKEN_NAMES, Theme};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// The first line of the saved settings.
pub const MAGIC: &str = "kai-appearance 1";
/// The settings, in the file's order.
pub const SETTINGS: [&str; 6] = [
    "theme",
    "token",
    "scale",
    "text_size",
    "reduced_motion",
    "legible",
];
/// The longest saved settings read.
pub const MAX_BYTES: usize = 2 * 1024;

/// Where themes come from: the shipped ones and the user's files.
pub trait Themes {
    fn theme(&self, name: &str) -> Option<Theme>;
}

/// The shipped themes only.
pub struct Shipped;

impl Themes for Shipped {
    fn theme(&self, name: &str) -> Option<Theme> {
        theme::shipped()
            .into_iter()
            .find(|theme| theme.name == name)
    }
}

/// What the user chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub theme: String,
    /// Colours over the theme, at most one a token, in [`Token::ALL`]'s order.
    pub tokens: Vec<(Token, u32)>,
    pub scale: u16,
    pub text_size: u16,
    pub reduced_motion: bool,
    pub legible: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: String::from("dark"),
            tokens: Vec::new(),
            scale: 100,
            text_size: 100,
            reduced_motion: false,
            legible: false,
        }
    }
}

/// Why a setting was refused, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub String);

fn refused(why: impl Into<String>) -> Refused {
    Refused(why.into())
}

fn token_named(name: &str) -> Option<Token> {
    TOKEN_NAMES
        .iter()
        .position(|n| *n == name)
        .map(|at| Token::ALL[at])
}

fn switch(value: &str) -> Result<bool, Refused> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(refused("it is `on` or `off`")),
    }
}

fn percent(value: &str, low: u16, high: u16, step: u16) -> Result<u16, Refused> {
    let digits = value.strip_suffix('%').unwrap_or(value);
    match digits.parse::<u16>() {
        Ok(n) if (low..=high).contains(&n) && n % step == 0 => Ok(n),
        _ => Err(refused(format!(
            "it is {low}% to {high}% in steps of {step}"
        ))),
    }
}

impl Appearance {
    /// The theme this ends in: the named theme, its colours, type and size.
    pub fn resolve(&self, themes: &dyn Themes) -> Result<Theme, Refused> {
        let base = themes
            .theme(&self.theme)
            .ok_or_else(|| refused(format!("there is no theme {:?}", self.theme)))?;
        let theme = self
            .tokens
            .iter()
            .fold(base, |theme, &(token, rgb)| theme.with_colour(token, rgb))
            .sized(self.legible, self.text_size);
        theme.check().map_err(Refused)?;
        Ok(theme)
    }

    /// `setting` set to `value`, checked; `self` is left as it was.
    pub fn set(&self, setting: &str, value: &str, themes: &dyn Themes) -> Result<Self, Refused> {
        let mut next = self.clone();
        match setting {
            "theme" => {
                if themes.theme(value).is_none() {
                    return Err(refused(format!("there is no theme {value:?}")));
                }
                // Colours chosen over one theme need not suit another.
                next.theme = String::from(value);
                next.tokens.clear();
            }
            "token" => {
                let (name, colour) = value.split_once(' ').unwrap_or((value, ""));
                let token =
                    token_named(name).ok_or_else(|| refused(format!("no token {name:?}")))?;
                next.tokens.retain(|(t, _)| *t != token);
                if !colour.is_empty() {
                    let rgb = theme::parse_colour(colour)
                        .ok_or_else(|| refused("a colour is `#RRGGBB`"))?;
                    next.tokens.push((token, rgb));
                    next.tokens
                        .sort_by_key(|(t, _)| Token::ALL.iter().position(|a| a == t));
                }
            }
            "scale" => next.scale = percent(value, 100, 300, 25)?,
            "text_size" => next.text_size = percent(value, 100, 200, 10)?,
            "reduced_motion" => next.reduced_motion = switch(value)?,
            "legible" => next.legible = switch(value)?,
            _ => return Err(refused(format!("no setting {setting:?}"))),
        }
        next.resolve(themes)?;
        Ok(next)
    }

    /// The saved settings.
    pub fn encode(&self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        let mut out = format!("{MAGIC}\ntheme = {}\n", self.theme);
        for (token, rgb) in &self.tokens {
            let at = Token::ALL.iter().position(|t| t == token).unwrap_or(0);
            let _ = writeln!(out, "token = {} #{rgb:06X}", TOKEN_NAMES[at]);
        }
        let _ = write!(
            out,
            "scale = {}\ntext_size = {}\nreduced_motion = {}\nlegible = {}\n",
            self.scale,
            self.text_size,
            on(self.reduced_motion),
            on(self.legible)
        );
        out
    }

    /// Reads saved settings, each through [`Appearance::set`], so what was
    /// saved and is no longer valid (a theme since deleted) is refused whole.
    pub fn decode(text: &str, themes: &dyn Themes) -> Result<Self, Refused> {
        if text.len() > MAX_BYTES {
            return Err(refused("the saved appearance is too long"));
        }
        let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
        if lines.next() != Some(MAGIC) {
            return Err(refused(format!("the first line is not {MAGIC:?}")));
        }
        let mut appearance = Appearance::default();
        let mut seen = [false; 6];
        for line in lines.filter(|l| !l.is_empty()) {
            let (setting, value) = line
                .split_once(" = ")
                .ok_or_else(|| refused("a line is not `setting = value`"))?;
            let at = SETTINGS
                .iter()
                .position(|s| *s == setting)
                .ok_or_else(|| refused(format!("no setting {setting:?}")))?;
            // Tokens may repeat, one each; every other setting comes once.
            if setting != "token" && core::mem::replace(&mut seen[at], true) {
                return Err(refused(format!("{setting} is given twice")));
            }
            appearance = appearance.set(setting, value, themes)?;
        }
        Ok(appearance)
    }

    /// What changed from `before` to `self`, for the reply and the log:
    /// `theme light (was dark)`.
    pub fn change(&self, before: &Appearance) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.theme != before.theme {
            parts.push(format!("theme {} (was {})", self.theme, before.theme));
        }
        if self.tokens != before.tokens {
            parts.push(String::from("theme colours"));
        }
        for (name, now, was) in [
            ("scale", self.scale, before.scale),
            ("text size", self.text_size, before.text_size),
        ] {
            if now != was {
                parts.push(format!("{name} {now}% (was {was}%)"));
            }
        }
        let on = |b: bool| if b { "on" } else { "off" };
        for (name, now, was) in [
            ("reduced motion", self.reduced_motion, before.reduced_motion),
            ("legible type", self.legible, before.legible),
        ] {
            if now != was {
                parts.push(format!("{name} {} (was {})", on(now), on(was)));
            }
        }
        match parts.is_empty() {
            true => String::from("nothing changed"),
            false => parts.join(", "),
        }
    }
}

/// Whether `rgb` as `token` would pass on `theme`'s grounds: what a colour
/// picker can show before the user chooses.
pub fn fits(theme: &Theme, token: Token, rgb: u32) -> bool {
    theme.with_colour(token, rgb).check().is_ok()
}
