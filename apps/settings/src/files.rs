//! The Settings app's own files in `/system/settings/<area>.cbor` (ADR 0004 §5):
//! each is `[schema, body]` in CBOR, the body a map with text keys, as the
//! first fixture there (`display.cbor`, `[1, {"scale": 2}]`) is. A reader takes
//! the keys it knows and skips the rest, so a later schema can add keys; a file
//! it cannot read at all gives the defaults.

use alloc::string::String;
use alloc::vec::Vec;
use minicbor::data::Type;
use minicbor::{Decoder, Encoder};

/// The schema every area's file is written with.
pub const SCHEMA: u32 = 1;

/// `appearance.cbor`: the theme by `look` palette name, and the two switches
/// ADR 0005 §8 names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub theme: String,
    pub legible: bool,
    pub reduced_motion: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: String::from(look::THEMES[0].name),
            legible: false,
            reduced_motion: false,
        }
    }
}

/// `apps.cbor`: the apps the user keeps running, which app nap never
/// suspends (founder, 2026-10-28; P3 task 12b). Sorted, each once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Apps {
    pub keep_running: Vec<String>,
}

/// `updates.cbor`: when an update may restart the machine by itself (ADR 0008
/// question 3, P4 plan question 10). `None`, the default, means only when the
/// user says "restart"; otherwise minutes after local midnight, start and end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Updates {
    pub restart_window: Option<(u16, u16)>,
}

/// Minutes in a day: a window's ends are below it.
pub const DAY_MINUTES: u16 = 24 * 60;

fn header(e: &mut Encoder<&mut Vec<u8>>, keys: u64) {
    // Writing to a Vec cannot fail.
    let _ = e
        .array(2)
        .and_then(|e| e.u32(SCHEMA))
        .and_then(|e| e.map(keys));
}

/// The body's map after a `[SCHEMA, ...]` header, as (key, decoder at its value).
fn body(bytes: &[u8], mut each: impl FnMut(&str, &mut Decoder<'_>) -> Option<()>) -> Option<()> {
    let mut d = Decoder::new(bytes);
    if d.array().ok()? != Some(2) || d.u32().ok()? != SCHEMA {
        return None;
    }
    let len = d.map().ok()??;
    for _ in 0..len {
        let key = d.str().ok()?;
        let at = d.position();
        if each(key, &mut d).is_none() {
            d.set_position(at);
            d.skip().ok()?;
        }
    }
    Some(())
}

impl Appearance {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = Encoder::new(&mut out);
        header(&mut e, 3);
        let _ = e
            .str("theme")
            .and_then(|e| e.str(&self.theme))
            .and_then(|e| e.str("legible"))
            .and_then(|e| e.bool(self.legible))
            .and_then(|e| e.str("reduced_motion"))
            .and_then(|e| e.bool(self.reduced_motion));
        out
    }

    /// The file's settings over the defaults; a theme `look` does not have is
    /// the default theme.
    pub fn decode(bytes: &[u8]) -> Self {
        let mut a = Appearance::default();
        body(bytes, |key, d| {
            match key {
                "theme" => {
                    let name = d.str().ok()?;
                    if look::THEMES.iter().any(|p| p.name == name) {
                        a.theme = String::from(name);
                    }
                }
                "legible" => a.legible = d.bool().ok()?,
                "reduced_motion" => a.reduced_motion = d.bool().ok()?,
                _ => return None,
            }
            Some(())
        });
        a
    }
}

/// Longest app name kept, as a supervisor service name may be.
pub const MAX_APP: usize = 64;
/// Most apps the keep-running list holds.
pub const MAX_KEPT: usize = 256;

impl Apps {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = Encoder::new(&mut out);
        header(&mut e, 1);
        let _ = e
            .str("keep_running")
            .and_then(|e| e.array(self.keep_running.len() as u64));
        for app in &self.keep_running {
            let _ = e.str(app);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Self {
        let mut apps = Apps::default();
        body(bytes, |key, d| {
            if key != "keep_running" || d.datatype().ok()? != Type::Array {
                return None;
            }
            let len = d.array().ok()??;
            for _ in 0..len {
                let name = d.str().ok()?;
                if !name.is_empty() && name.len() <= MAX_APP {
                    apps.set(name, true);
                }
            }
            Some(())
        });
        apps
    }

    pub fn keeps(&self, app: &str) -> bool {
        self.keep_running.iter().any(|a| a == app)
    }

    /// Turns `app`'s keep-running switch on or off; false if nothing changed.
    pub fn set(&mut self, app: &str, on: bool) -> bool {
        match (
            self.keep_running.binary_search_by(|a| a.as_str().cmp(app)),
            on,
        ) {
            (Ok(_), true) | (Err(_), false) => false,
            (Err(_), true) if self.keep_running.len() >= MAX_KEPT => false,
            (Err(at), true) => {
                self.keep_running.insert(at, String::from(app));
                true
            }
            (Ok(at), false) => {
                self.keep_running.remove(at);
                true
            }
        }
    }
}

impl Updates {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = Encoder::new(&mut out);
        header(&mut e, 1);
        let _ = e.str("restart_window");
        let _ = match self.restart_window {
            Some((start, end)) => e
                .array(2)
                .and_then(|e| e.u16(start))
                .and_then(|e| e.u16(end)),
            None => e.null(),
        };
        out
    }

    /// A window whose ends are not minutes of a day, or are the same minute,
    /// is no window.
    pub fn decode(bytes: &[u8]) -> Self {
        let mut u = Updates::default();
        body(bytes, |key, d| {
            if key != "restart_window" {
                return None;
            }
            if d.datatype().ok()? == Type::Null {
                d.null().ok()?;
                return Some(());
            }
            if d.array().ok()? != Some(2) {
                return None;
            }
            let (start, end) = (d.u16().ok()?, d.u16().ok()?);
            u.restart_window = window(start, end);
            Some(())
        });
        u
    }

    /// Whether minute `now` of the day falls in the window; a window may run
    /// past midnight.
    pub fn allows(&self, now: u16) -> bool {
        match self.restart_window {
            None => false,
            Some((start, end)) if start < end => (start..end).contains(&now),
            Some((start, end)) => now >= start || now < end,
        }
    }
}

/// `(start, end)` as a window, if both are minutes of a day and differ.
pub fn window(start: u16, end: u16) -> Option<(u16, u16)> {
    (start < DAY_MINUTES && end < DAY_MINUTES && start != end).then_some((start, end))
}
