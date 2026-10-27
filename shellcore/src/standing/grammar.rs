//! A standing instruction in the user's words, as `rules` lists it and
//! `rules_add` takes it:
//!
//! ```text
//! on minimize(notes) -> app_suspend(notes) until restore(notes)
//! on idle(15 min) -> app_suspend(mail)
//! on time(22:00) -> app_suspend(game)
//! ```
//!
//! The text is the user's line, but it is read as hostile all the same: anything
//! out of shape is refused, never guessed at. What reads describes itself as the
//! same text, with its spaces made single.

use super::{Kind, When, is_app, kind_word, when_text};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A rule before it is made: what the ask card shows, in code's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub on: When,
    pub action: String,
    pub target: String,
    pub until: Option<When>,
}

impl Draft {
    /// The rule as `rules` lists it, and as the ask card shows it.
    pub fn describe(&self) -> String {
        let mut text = alloc::format!(
            "on {} -> {}({})",
            when_text(&self.on),
            self.action,
            self.target
        );
        if let Some(until) = &self.until {
            text.push_str(" until ");
            text.push_str(&when_text(until));
        }
        text
    }
}

/// Longest rule text read: a rule is one short line.
pub const MAX_TEXT: usize = 160;

/// Reads `on <event> -> <action>(<app>) [until <event>]`.
pub fn parse(text: &str) -> Result<Draft, &'static str> {
    if text.len() > MAX_TEXT {
        return Err("a rule is one short line");
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    // `idle(15 min)` is the one event with a space inside it.
    let words = join_idle(&words);
    let (on, action, until) = match words.as_slice() {
        [w, event, arrow, call] if w == "on" && arrow == "->" => {
            (event.as_str(), call.as_str(), None)
        }
        [w, event, arrow, call, u, until] if w == "on" && arrow == "->" && u == "until" => {
            (event.as_str(), call.as_str(), Some(until.as_str()))
        }
        _ => return Err("a rule is: on <event> -> <action>(<app>) [until <event>]"),
    };
    let (name, target) = call_of(action)?;
    Ok(Draft {
        on: event(on)?,
        action: name.to_string(),
        target: target.to_string(),
        until: until.map(event).transpose()?,
    })
}

/// The words, with `idle(15` and `min)` made one.
fn join_idle(words: &[&str]) -> Vec<String> {
    let mut out = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        match (words[i].starts_with("idle("), words.get(i + 1)) {
            (true, Some(&"min)")) => {
                out.push(alloc::format!("{} min)", words[i]));
                i += 2;
            }
            _ => {
                out.push(words[i].to_string());
                i += 1;
            }
        }
    }
    out
}

/// `app_suspend(notes)` as (`app_suspend`, `notes`).
fn call_of(text: &str) -> Result<(&str, &str), &'static str> {
    let (name, rest) = text.split_once('(').ok_or("an action is <row>(<app>)")?;
    let target = rest.strip_suffix(')').ok_or("an action is <row>(<app>)")?;
    let row = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    match (row, is_app(target)) {
        (true, true) => Ok((name, target)),
        (false, _) => Err("an action's row is lowercase, as app_suspend"),
        (true, false) => Err("an app is named [a-z][a-z0-9-], at most 12 bytes"),
    }
}

/// `minimize(notes)`, `idle(15 min)` or `time(22:00)`.
fn event(text: &str) -> Result<When, &'static str> {
    let (kind, rest) = text.split_once('(').ok_or("an event is <kind>(<app>)")?;
    let inside = rest.strip_suffix(')').ok_or("an event is <kind>(<app>)")?;
    let when = |kind: Kind, app: &str| When {
        kind,
        app: app.to_string(),
    };
    match kind {
        "idle" => {
            let minutes = inside
                .strip_suffix(" min")
                .and_then(number)
                .filter(|m| (1..=24 * 60).contains(m))
                .ok_or("idle is idle(<1 to 1440> min)")?;
            Ok(when(Kind::Idle(minutes), ""))
        }
        "time" => {
            let (h, m) = inside.split_once(':').ok_or("a time is time(HH:MM)")?;
            let ok = h.len() == 2 && m.len() == 2;
            match (ok, number(h), number(m)) {
                (true, Some(h), Some(m)) if h < 24 && m < 60 => {
                    Ok(when(Kind::Time(h * 60 + m), ""))
                }
                _ => Err("a time is time(HH:MM), 00:00 to 23:59"),
            }
        }
        _ => {
            let kind = [Kind::Minimize, Kind::Restore, Kind::Focus, Kind::Close]
                .into_iter()
                .find(|k| kind_word(*k) == kind)
                .ok_or("an event is minimize, restore, focus, close, idle or time")?;
            match is_app(inside) {
                true => Ok(when(kind, inside)),
                false => Err("an app is named [a-z][a-z0-9-], at most 12 bytes"),
            }
        }
    }
}

/// Decimal digits only, no sign, at most four.
fn number(text: &str) -> Option<u16> {
    let digits = !text.is_empty() && text.len() <= 4 && text.bytes().all(|b| b.is_ascii_digit());
    digits.then(|| text.parse().ok()).flatten()
}
