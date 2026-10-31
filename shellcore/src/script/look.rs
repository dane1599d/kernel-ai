//! `appearance_set(setting, value)` as a script call (P4 task 8): one setting,
//! checked by `look`, saved through the host, with its old value as the undo.

use super::{RhaiResult, Tx, rhai_error};
use crate::Host;
use alloc::rc::Rc;
use rhai::Engine;

pub(super) fn bind(engine: &mut Engine, h: Rc<dyn Host>, tx: Tx) {
    let name = "appearance_set";
    engine.register_fn(name, move |setting: &str, value: &str| -> RhaiResult<()> {
        tx.approved(name, &[setting, value])?;
        let (change, _) =
            crate::appearance::changed(h.as_ref(), setting, value).map_err(rhai_error)?;
        tx.changed(change);
        Ok(())
    });
}
