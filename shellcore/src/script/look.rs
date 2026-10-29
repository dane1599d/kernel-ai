//! `appearance_set(setting, value)` as a script call (P4 task 8): one setting,
//! checked by `look`, saved through the host, with its old value as the undo.

use super::{RhaiResult, Tx, rhai_error};
use crate::Host;
use crate::undo::{Done, Inverse, Name};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use rhai::Engine;

pub(super) fn bind(engine: &mut Engine, h: Rc<dyn Host>, tx: Tx) {
    let name = "appearance_set";
    engine.register_fn(name, move |setting: &str, value: &str| -> RhaiResult<()> {
        tx.approved(name, &[setting, value])?;
        let fail = |why: String| rhai_error(format!("{name}: {why}"));
        let (before, words) =
            crate::appearance::set(h.as_ref(), setting, value).map_err(rhai_error)?;
        // A colour's undo clears it: its old colour is not held.
        let before = match setting {
            "token" => String::from(value.split(' ').next().unwrap_or(value)),
            _ => before,
        };
        let after = crate::appearance::value_of(&crate::appearance::current(h.as_ref()), setting);
        let named = |text: &str| {
            Name::new(text).ok_or_else(|| fail(format!("{text:?} is too long to undo")))
        };
        tx.changed(Done {
            action: name.into(),
            owner: "desktop".into(),
            res: format!("appearance {setting}"),
            before: before.clone(),
            after: words,
            inverse: Some(Inverse::Appearance {
                setting: named(setting)?,
                before: named(&before)?,
                after: named(&after)?,
            }),
        });
        Ok(())
    });
}
