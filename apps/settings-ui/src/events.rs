//! The window's events: each resolved against what the tree last built showed
//! (`View`'s rows), and the draw that follows.

use crate::rows::Slots;
use crate::tree::{Change, Entry, diff};
use crate::*;
use comp_settings::files::Appearance;
use comp_settings::{Intent, Page, Settings};
use uikit::widget::{NodeId, UiEvent, Value};

/// `id` as the `i`th of the rows starting at `base`, if it is one.
fn nth(id: NodeId, base: NodeId) -> Option<usize> {
    id.checked_sub(base)
        .map(|i| i as usize)
        .filter(|&i| i < MAX_ROWS)
}

/// The changes that draw `new` over `old`: [`diff`], with the control the last
/// change came from put again whatever it holds. `uikit` moved that control
/// itself, and the model may have refused the change or not taken it yet.
pub fn redraw(old: &[Entry], new: &[Entry], view: &mut View) -> Vec<Change> {
    match view.touched.take() {
        Some(id) => {
            let old: Vec<Entry> = old.iter().filter(|e| e.id != id).cloned().collect();
            diff(&old, new)
        }
        None => diff(old, new),
    }
}

/// Handles one event: changes `view` for a page or a selection, and answers a
/// choice with the intent to carry out, resolved against what the tree last
/// built showed. True if the window must be drawn again ([`redraw`]).
pub fn on_ui(settings: &Settings, view: &mut View, e: &UiEvent) -> (bool, Option<Intent>) {
    match e {
        UiEvent::Changed(TABS, Value::Index(i)) if (*i as usize) < Page::ALL.len() => {
            *view = View::page(*i as usize);
            (true, None)
        }
        UiEvent::Changed(APP_LIST, Value::Index(i)) => {
            view.app = view.shown.apps.get(*i as usize).cloned();
            view.shown.grants = Slots::default();
            (true, None)
        }
        UiEvent::Pressed(THEME) => (false, Some(settings.next_theme())),
        UiEvent::Pressed(id) => (false, pressed(settings, view, *id)),
        UiEvent::Changed(id, value) => {
            view.touched = Some(*id);
            (true, changed(settings, view, *id, value))
        }
        _ => (false, None),
    }
}

fn changed(settings: &Settings, view: &View, id: NodeId, value: &Value) -> Option<Intent> {
    match (id, value) {
        (LEGIBLE | MOTION, Value::On(on)) => {
            let mut a: Appearance = settings.appearance.clone();
            if id == LEGIBLE {
                a.legible = *on;
            } else {
                a.reduced_motion = *on;
            }
            (a != settings.appearance).then_some(Intent::Appearance(a))
        }
        (PROFILES, Value::Index(i)) => {
            let name = view.shown.profiles.get(*i as usize)?;
            let known = settings.profiles.contains(name);
            (known && Some(name) != settings.profile.as_ref())
                .then(|| Intent::UseProfile(name.clone()))
        }
        (WINDOW_ON, Value::On(on)) => settings.restart_window(on.then_some(DEFAULT_WINDOW)),
        (START | END, Value::Number(n)) => {
            let (start, end) = settings.updates.restart_window?;
            let n = u16::try_from(*n).ok()?;
            let w = if id == START { (n, end) } else { (start, n) };
            settings.restart_window(Some(w))
        }
        (_, Value::On(on)) => {
            let app = view.shown.keep.get(nth(id, KEEP)?)?;
            settings.keep_running(app, *on)
        }
        _ => None,
    }
}

fn pressed(settings: &Settings, view: &View, id: NodeId) -> Option<Intent> {
    if let Some(slot) = nth(id, REVOKE) {
        let app = view.app.as_deref()?;
        return settings.revoke(app, view.shown.grants.get(slot)?);
    }
    if let Some(slot) = nth(id, RESTORE) {
        let app = view.app.as_deref()?;
        return settings.restore(app, view.shown.grants.get(slot)?);
    }
    let rule = *view.shown.rules.get(nth(id, REMOVE)?)?;
    settings
        .rules
        .iter()
        .any(|r| r.id == rule)
        .then_some(Intent::RemoveRule(rule))
}
