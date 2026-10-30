//! `appearance_set(setting, value)` (P4 task 8, ADR 0005 §8): the user's
//! appearance, kept by the host as `look::appearance`'s `kai-appearance 1`
//! text. A setting is checked by `look` (a theme that exists, contrast kept);
//! `undo` sets the setting's old value back, if the setting is still what the
//! change left.

use crate::Host;
use crate::actlog::Change;
use crate::undo::{Inverse, Name, Refused};
use alloc::format;
use alloc::string::{String, ToString};
use look::appearance::{Appearance, Shipped};

/// The appearance the host keeps; the default when it keeps none, or one out
/// of shape (which is reported and left alone until a change replaces it).
pub(crate) fn current(host: &dyn Host) -> Appearance {
    match host.appearance_read() {
        Some(text) => Appearance::decode(&text, &Shipped).unwrap_or_else(|why| {
            host.audit(&format!("the saved appearance is refused: {}", why.0));
            Appearance::default()
        }),
        None => Appearance::default(),
    }
}

/// `setting`'s value in `appearance`, as `appearance_set` takes it.
pub(crate) fn value_of(appearance: &Appearance, setting: &str) -> String {
    let on = |b: bool| String::from(if b { "on" } else { "off" });
    match setting {
        "theme" => appearance.theme.clone(),
        "scale" => appearance.scale.to_string(),
        "text_size" => appearance.text_size.to_string(),
        "reduced_motion" => on(appearance.reduced_motion),
        "legible" => on(appearance.legible),
        // A colour token's value names the token; its old colour is in the
        // saved text, which `undo` cannot hold, so it only clears it.
        _ => String::new(),
    }
}

/// Sets `setting` to `value` and saves it: the value it replaced and the
/// reply's words.
pub(crate) fn set(host: &dyn Host, setting: &str, value: &str) -> Result<(String, String), String> {
    let before = current(host);
    let after = before
        .set(setting, value, &Shipped)
        .map_err(|why| format!("appearance_set: {}", why.0))?;
    host.appearance_write(&after.encode())?;
    Ok((value_of(&before, setting), after.change(&before)))
}

/// Sets `setting` to `value` and saves it, as `appearance_set` does from a
/// script or a Settings click: the change to log, with its undo. A colour's
/// undo clears it: its old colour is not held.
pub(crate) fn changed(host: &dyn Host, setting: &str, value: &str) -> Result<Change, String> {
    let (before, words) = set(host, setting, value)?;
    let before = match setting {
        "token" => String::from(value.split(' ').next().unwrap_or(value)),
        _ => before,
    };
    let after = value_of(&current(host), setting);
    let named = |text: &str| {
        Name::new(text).ok_or_else(|| format!("appearance_set: {text:?} is too long to undo"))
    };
    Ok(Change {
        action: String::from("appearance_set"),
        owner: String::from("desktop"),
        res: format!("appearance {setting}"),
        before: before.clone(),
        after: words,
        inverse: Some(Inverse::Appearance {
            setting: named(setting)?,
            before: named(&before)?,
            after: named(&after)?,
        }),
    })
}

/// `undo`: puts `setting` back to `before`, unless something else set it
/// since (`now` is what the change left), or `anyway`.
pub(crate) fn revert(
    host: &dyn Host,
    setting: Name,
    before: Name,
    now: Name,
    anyway: bool,
) -> Result<String, Refused> {
    let current = current(host);
    let is = value_of(&current, setting.as_str());
    if !anyway && is != now.as_str() {
        return Err(Refused::Moved(format!("{} {is}", setting.as_str())));
    }
    set(host, setting.as_str(), before.as_str())
        .map(|(_, words)| words)
        .map_err(Refused::Failed)
}
