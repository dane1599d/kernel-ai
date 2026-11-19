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
//! - `time_zone`: the tray clock's fixed offset from UTC, `UTC` or `+05:30`
//!   (`-9`, `+2`), from -12:00 to +14:00 in 15-minute steps. No daylight
//!   saving: the person changes it. It is a file and plan-line setting only
//!   until the agent's row and the Settings app list it ([`FILE_SETTINGS`]).
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
/// Every setting the saved file may hold: [`SETTINGS`], which the agent's
/// `appearance_set` row names, and `time_zone`, which it does not yet.
pub const FILE_SETTINGS: [&str; 7] = [
    "theme",
    "token",
    "scale",
    "text_size",
    "reduced_motion",
    "legible",
    "time_zone",
];
/// The earliest and latest `time_zone`, in minutes from UTC.
pub const ZONE_MIN: i16 = -12 * 60;
pub const ZONE_MAX: i16 = 14 * 60;
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
    /// The tray clock's offset from UTC in minutes, a multiple of 15 from
    /// [`ZONE_MIN`] to [`ZONE_MAX`]; 0 is UTC.
    pub time_zone_min: i16,
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
            time_zone_min: 0,
        }
    }
}

/// Why a setting was refused, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused(pub String);

fn refused(why: impl Into<String>) -> Refused {
    Refused(why.into())
}

/// A token's name in a file: `muted`.
fn token_name(token: Token) -> &'static str {
    let at = Token::ALL.iter().position(|t| *t == token).unwrap_or(0);
    TOKEN_NAMES[at]
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

/// A `time_zone` value: `UTC`, or a sign, hours and optional `:MM`.
fn zone(value: &str) -> Result<i16, Refused> {
    let bad = || refused("it is UTC or an offset like +05:30, from -12:00 to +14:00");
    if value == "UTC" {
        return Ok(0);
    }
    let (sign, rest) = match value.as_bytes().first() {
        Some(b'+') => (1i16, &value[1..]),
        Some(b'-') => (-1i16, &value[1..]),
        _ => return Err(bad()),
    };
    let (hours, minutes) = rest.split_once(':').unwrap_or((rest, "0"));
    let digits = |s: &str, most: usize| {
        (!s.is_empty() && s.len() <= most && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<i16>().ok())
            .flatten()
    };
    let (Some(h), Some(m)) = (digits(hours, 2), digits(minutes, 2)) else {
        return Err(bad());
    };
    let total = sign * (h * 60 + m);
    match m < 60 && m % 15 == 0 && (ZONE_MIN..=ZONE_MAX).contains(&total) {
        true => Ok(total),
        false => Err(bad()),
    }
}

/// An offset as a file writes it: `UTC`, `+02:00`, `-09:30`.
pub fn zone_text(minutes: i16) -> String {
    match minutes {
        0 => String::from("UTC"),
        m => format!(
            "{}{:02}:{:02}",
            if m < 0 { '-' } else { '+' },
            m.abs() / 60,
            m.abs() % 60
        ),
    }
}

impl Appearance {
    /// The theme this ends in: the named theme, its colours, type and size.
    pub fn resolve(&self, themes: &dyn Themes) -> Result<Theme, Refused> {
        let theme = self.build(themes)?;
        theme.check().map_err(Refused)?;
        Ok(theme)
    }

    /// [`Appearance::resolve`], before the theme is checked.
    fn build(&self, themes: &dyn Themes) -> Result<Theme, Refused> {
        let base = themes
            .theme(&self.theme)
            .ok_or_else(|| refused(format!("there is no theme {:?}", self.theme)))?;
        Ok(self
            .tokens
            .iter()
            .fold(base, |theme, &(token, rgb)| theme.with_colour(token, rgb))
            .sized(self.legible, self.text_size))
    }

    /// `setting` set to `value`, checked; `self` is left as it was.
    pub fn set(&self, setting: &str, value: &str, themes: &dyn Themes) -> Result<Self, Refused> {
        let next = self.changed(setting, value, themes)?;
        next.resolve(themes)?;
        Ok(next)
    }

    /// `setting` set to `value` as far as the value itself goes: a name that
    /// exists, a colour or a number in range. Not yet checked against the theme
    /// it ends in.
    fn changed(&self, setting: &str, value: &str, themes: &dyn Themes) -> Result<Self, Refused> {
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
            "time_zone" => next.time_zone_min = zone(value)?,
            _ => return Err(refused(format!("no setting {setting:?}"))),
        }
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
        // Written only when set, so a saved file without one stays as it was.
        if self.time_zone_min != 0 {
            let _ = writeln!(out, "time_zone = {}", zone_text(self.time_zone_min));
        }
        out
    }

    /// Reads saved settings, the theme they end in checked once, so what was
    /// saved and is no longer valid (a theme since deleted) is refused whole.
    /// The one exception is [`Appearance::decode_audited`]'s.
    pub fn decode(text: &str, themes: &dyn Themes) -> Result<Self, Refused> {
        Self::decode_audited(text, themes).map(|(appearance, _)| appearance)
    }

    /// [`Appearance::decode`], and what it let go. A save made before `shell`
    /// and `hover` existed may hold a colour that cleared every ground it knew
    /// and falls short on one of these (light `muted #666666` is 4.32:1 on
    /// `hover`). That one colour is dropped, with a line saying which and why
    /// for the host's audit log, and the rest is kept; refusing the whole save
    /// would reset the person's theme, scale and legible type to the defaults.
    /// A colour short on any earlier ground was short before, and still refuses
    /// the whole save. A setting changed now with [`Appearance::set`] is refused
    /// on `shell` and `hover` like on any other ground.
    ///
    /// Every saved colour is applied before the theme is checked, so a colour is
    /// judged against the others saved with it and not against the defaults the
    /// file has not yet reached: a file lists `hover` before `error`, and a
    /// `hover` that holds only with the saved `error` is kept.
    pub fn decode_audited(text: &str, themes: &dyn Themes) -> Result<(Self, Vec<String>), Refused> {
        if text.len() > MAX_BYTES {
            return Err(refused("the saved appearance is too long"));
        }
        let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
        if lines.next() != Some(MAGIC) {
            return Err(refused(format!("the first line is not {MAGIC:?}")));
        }
        let mut appearance = Appearance::default();
        let mut seen = [false; FILE_SETTINGS.len()];
        for line in lines.filter(|l| !l.is_empty()) {
            let (setting, value) = line
                .split_once(" = ")
                .ok_or_else(|| refused("a line is not `setting = value`"))?;
            let at = FILE_SETTINGS
                .iter()
                .position(|s| *s == setting)
                .ok_or_else(|| refused(format!("no setting {setting:?}")))?;
            // Tokens may repeat, one each; every other setting comes once.
            if setting != "token" && core::mem::replace(&mut seen[at], true) {
                return Err(refused(format!("{setting} is given twice")));
            }
            appearance = appearance.changed(setting, value, themes)?;
        }
        // Every saved setting is in before the theme is checked, once: a colour
        // is judged against the others saved with it, whatever order the file
        // lists them in (`hover` comes before `error`, which it may need).
        let mut dropped = Vec::new();
        while let Some(short) = appearance.build(themes)?.shortfall() {
            // Only a shortfall on `shell` or `hover` is let go, and only by
            // dropping a colour the save set: the one that falls short, or else
            // the ground it falls short on. Any other failure is the theme's, or
            // was one before, and the check below refuses the whole save.
            let held = |token| appearance.tokens.iter().find(|(t, _)| *t == token);
            let culprit = [short.token, short.ground]
                .into_iter()
                .find_map(held)
                .filter(|_| short.is_on_a_desktop_ground());
            let Some(&(token, rgb)) = culprit else {
                break;
            };
            dropped.push(format!(
                "the saved colour {} #{rgb:06X} is dropped: {}",
                token_name(token),
                short.message()
            ));
            appearance.tokens.retain(|(t, _)| *t != token);
        }
        appearance.resolve(themes)?;
        Ok((appearance, dropped))
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
        if self.time_zone_min != before.time_zone_min {
            parts.push(format!(
                "time zone {} (was {})",
                zone_text(self.time_zone_min),
                zone_text(before.time_zone_min)
            ));
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
