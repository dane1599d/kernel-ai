//! The agent's window rows on the strict engine (P3 task 10): each asks
//! `desktop` through [`Host::ui`]. `ui_windows` hands back the list and
//! `ui_view` the view as `Data`; each change is journalled with its inverse, so
//! `undo` closes what `ui_open` opened, reopens what `window_close` closed and
//! puts focus back. An optional argument left out is a shorter call.

use super::{RhaiResult, Tx, rhai_error};
use crate::ui::{UiCall, action_bit, parse_windows};
use crate::undo::{Done, Inverse, Name};
use crate::{Host, data};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use rhai::{Array, Dynamic, Engine, Map};

pub(super) fn bind(engine: &mut Engine, name: &'static str, h: Rc<dyn Host>, tx: Tx) -> bool {
    let fail = move |why: String| rhai_error(format!("{name}: {why}"));
    let name_of = move |text: &str| {
        Name::new(text).ok_or_else(|| rhai_error(format!("{name}: {text} is too long a name")))
    };
    match name {
        "ui_windows" => {
            engine.register_fn(name, move || -> RhaiResult<Array> {
                tx.approved(name, &[])?;
                let list = h
                    .ui(&UiCall::Windows)
                    .and_then(|text| parse_windows(&text))
                    .map_err(fail)?;
                Ok(list
                    .into_iter()
                    .map(|w| {
                        let mut map = Map::new();
                        map.insert("id".into(), w.id.into());
                        map.insert("app".into(), w.app.into());
                        map.insert("title".into(), w.title.into());
                        Dynamic::from_map(map)
                    })
                    .collect())
            });
        }
        "ui_view" => {
            engine.register_fn(name, move |window: &str| -> RhaiResult<data::Data> {
                tx.approved(name, &[window])?;
                h.ui(&UiCall::View(window.into()))
                    .map(|view| data::Data::new("ui", view))
                    .map_err(fail)
            });
        }
        "ui_open" => {
            let open = move |app: &str, arg: Option<&str>| -> RhaiResult<()> {
                let live: &[&str] = match arg {
                    Some(arg) => &[app, arg],
                    None => &[app],
                };
                tx.approved(name, live)?;
                let call = UiCall::Open {
                    app: app.into(),
                    arg: arg.map(String::from),
                };
                let answer = h.ui(&call).map_err(fail)?;
                // A window the user had only took focus: undo moves focus back
                // and never closes it.
                let done = match crate::ui::opened(&answer, app) {
                    Some(Inverse::Refocus { to, from }) => Done {
                        action: name.into(),
                        owner: "desktop".into(),
                        res: String::from("keyboard focus"),
                        before: format!("on {}", to.as_str()),
                        after: format!("on {}", from.as_str()),
                        inverse: Some(Inverse::Refocus { to, from }),
                    },
                    inverse => Done {
                        inverse,
                        ..window_done(name, app, "closed", "open", Inverse::Reopen(name_of(app)?))
                    },
                };
                tx.changed(done);
                Ok(())
            };
            let again = open.clone();
            engine.register_fn(name, move |app: &str| open(app, None));
            engine.register_fn(name, move |app: &str, arg: &str| again(app, Some(arg)));
        }
        "ui_focus" => {
            engine.register_fn(name, move |window: &str| -> RhaiResult<()> {
                tx.approved(name, &[window])?;
                let before = h.ui(&UiCall::Focus(window.into())).map_err(fail)?;
                let inverse = Inverse::Refocus {
                    to: name_of(&before)?,
                    from: name_of(window)?,
                };
                let after = format!("on {window}");
                let was = format!("on {before}");
                tx.changed(Done {
                    action: name.into(),
                    owner: "desktop".into(),
                    res: String::from("keyboard focus"),
                    before: was,
                    after,
                    inverse: Some(inverse),
                });
                Ok(())
            });
        }
        "window_close" => {
            engine.register_fn(name, move |window: &str| -> RhaiResult<()> {
                tx.approved(name, &[window])?;
                let app = h.ui(&UiCall::Close(window.into())).map_err(fail)?;
                let app = if app.is_empty() {
                    String::from(window)
                } else {
                    app
                };
                tx.changed(window_done(
                    name,
                    &app,
                    "open",
                    "closed",
                    Inverse::Reopen(name_of(&app)?),
                ));
                Ok(())
            });
        }
        "ui_act" => {
            let act =
                move |window: &str, node: &str, action: &str, value: &str| -> RhaiResult<()> {
                    let live: &[&str] = match value {
                        "" => &[window, node, action],
                        value => &[window, node, action, value],
                    };
                    tx.approved(name, live)?;
                    let call = UiCall::Act {
                        window: window.into(),
                        node: node.into(),
                        action: action_bit(action).map_err(fail)?,
                        value: value.into(),
                    };
                    h.ui(&call).map_err(fail)?;
                    let after = match value {
                        "" => String::from(action),
                        value => format!("{action} {value:?}"),
                    };
                    tx.changed(Done {
                        action: name.into(),
                        owner: "desktop".into(),
                        res: format!("control {node} in {window}"),
                        before: String::new(),
                        after,
                        inverse: None,
                    });
                    Ok(())
                };
            let again = act.clone();
            engine.register_fn(name, move |w: &str, n: &str, a: &str| act(w, n, a, ""));
            engine.register_fn(name, move |w: &str, n: &str, a: &str, v: &str| {
                again(w, n, a, v)
            });
        }
        _ => return false,
    }
    true
}

/// A window opened or closed: `window notes closed (was open)`.
fn window_done(
    action: &'static str,
    app: &str,
    before: &str,
    after: &str,
    inverse: Inverse,
) -> Done {
    Done {
        action: action.into(),
        owner: "desktop".into(),
        res: format!("window {app}"),
        before: before.into(),
        after: after.into(),
        inverse: Some(inverse),
    }
}
