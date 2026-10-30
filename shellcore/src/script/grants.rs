//! The grant rows on the strict engine (P4 task 7): `grant_revoke` withholds a
//! row an app declares for its effects and `grant_restore` gives it back, each
//! journalled with the other as its undo. Neither changes anything when the
//! row is already as asked, so neither is logged then.

use super::{RhaiResult, Tx, rhai_error};
use crate::Host;
use crate::undo::{Done, Inverse, Name};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use rhai::Engine;

pub(super) fn bind(engine: &mut Engine, name: &'static str, h: Rc<dyn Host>, tx: Tx) -> bool {
    let withheld = match name {
        "grant_revoke" => true,
        "grant_restore" => false,
        _ => return false,
    };
    engine.register_fn(name, move |app: &str, row: &str| -> RhaiResult<()> {
        tx.approved(name, &[app, row])?;
        let fail = |why: String| rhai_error(format!("{name}: {why}"));
        let (Some(app_name), Some(row_name)) = (Name::new(app), Name::new(row)) else {
            return Err(fail(format!("{app} {row} is too long a name")));
        };
        let changed = match withheld {
            true => crate::grants::revoke(h.as_ref(), app, row),
            false => crate::grants::set(h.as_ref(), app, row, false),
        }
        .map_err(fail)?;
        if changed {
            let (before, after) = match withheld {
                true => ("allowed", "withheld"),
                false => ("withheld", "allowed"),
            };
            tx.changed(Done {
                action: name.into(),
                owner: String::from("shell"),
                res: format!("{app}'s effects calling {row}"),
                before: before.into(),
                after: after.into(),
                inverse: Some(Inverse::Grant {
                    app: app_name,
                    row: row_name,
                    withheld: !withheld,
                }),
            });
        }
        Ok(())
    });
    true
}
