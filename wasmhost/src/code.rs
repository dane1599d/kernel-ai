//! `kai:code@0.1.0` for `lifecycle 2`: the code view. `put` is served with
//! `kai:ui/view`'s functions by the instance's `uikit::Ui` (`ui.rs`); this module
//! lowers what `uikit` hands back, a key, a click, the wheel and the size the
//! view shows, to the app's `code-input` exports. A view the app has no export
//! for is told nothing.

use crate::canonical::CoreInstance;
use crate::tables;
use alloc::string::String;
use alloc::vec;
use appabi::Val;
use livecore::{Fault, Reason};
use uikit::widget::UiEvent;
use wasmi::Module;

/// The interface whose `put` this host serves, beside `kai:ui/view`.
pub(crate) const CODE: &str = "kai:code/code";
/// The exports the view's events go to.
pub(crate) const INPUT: &str = "kai:code/code-input";

/// The `code-input` exports a module has, by name.
#[derive(Default)]
pub(crate) struct CodeExports {
    key: Option<String>,
    click: Option<String>,
    scroll: Option<String>,
    size: Option<String>,
}

impl CodeExports {
    pub(crate) fn of(module: &Module) -> Self {
        let find = |name: &str| {
            tables::EXPORTS
                .iter()
                .find(|f| f.interface == INPUT && f.name == name)
                .and_then(|f| crate::canonical::export_for(module, f))
        };
        Self {
            key: find("on-key"),
            click: find("on-click"),
            scroll: find("on-scroll"),
            size: find("on-size"),
        }
    }

    fn get(&self, name: &str) -> Option<&String> {
        match name {
            "on-key" => self.key.as_ref(),
            "on-click" => self.click.as_ref(),
            "on-scroll" => self.scroll.as_ref(),
            _ => self.size.as_ref(),
        }
    }
}

impl CoreInstance {
    fn code_call(&mut self, name: &str, args: &[Val]) -> Result<(), Fault> {
        let Some(export) = self.code.get(name).cloned() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == INPUT && f.name == name)
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:code/code-input table"))?;
        livecore::Instance::call(self, &entry.func(export), args)?;
        Ok(())
    }

    /// Gives the app a code view's key, click or wheel; any other event is not
    /// the view's and is ignored.
    pub(crate) fn code_event(&mut self, window: u32, e: &UiEvent) -> Result<(), Fault> {
        match e {
            UiEvent::Key {
                node,
                name,
                text,
                mods,
            } => self.code_call(
                "on-key",
                &[
                    Val::U32(window),
                    Val::U32(*node),
                    Val::Record(vec![
                        Val::String(name.clone()),
                        Val::String(text.clone()),
                        Val::U8(*mods),
                    ]),
                ],
            ),
            UiEvent::Click {
                node,
                row,
                cell,
                mods,
            } => self.code_call(
                "on-click",
                &[
                    Val::U32(window),
                    Val::U32(*node),
                    Val::Record(vec![Val::U32(*row), Val::U32(*cell), Val::U8(*mods)]),
                ],
            ),
            UiEvent::Scroll { node, rows } => self.code_call(
                "on-scroll",
                &[Val::U32(window), Val::U32(*node), Val::S32(*rows)],
            ),
            _ => Ok(()),
        }
    }

    /// Tells the app how many rows and cells its code view `node` shows now
    /// (`Update::CodeSize`); comphost gives it in a call of its own.
    pub fn code_size(
        &mut self,
        window: u32,
        node: u32,
        rows: u16,
        cells: u16,
    ) -> Result<(), Fault> {
        self.code_call(
            "on-size",
            &[
                Val::U32(window),
                Val::U32(node),
                Val::Record(vec![Val::U16(rows), Val::U16(cells)]),
            ],
        )
    }
}
