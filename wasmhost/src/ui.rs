//! `kai:ui@0.1.0` for `lifecycle 2` (ADR 0005 section 4): `view` served by a
//! `uikit::Ui` the instance owns, and `input` delivered to the app's `on-ui`.
//! A committed frame is kept as an [`Update`] until comphost takes it, so no
//! pixels and no window state enter the app's memory. Text is measured with
//! the measure comphost sets (`gfx2d`'s, through `uipaint`), or `uikit`'s
//! `FixedAdvance` until it does.

use crate::canonical::CoreInstance;
use crate::instance::HostState;
use crate::link::WitFunc;
use crate::tables;
use crate::uival::{self, bool_of, layout, str_of, u16_of, u32_of, widget};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use appabi::{Core, Guest, Trap, Val};
use livecore::{Fault, Reason};
use uikit::a11y::Action;
use uikit::geom::Rect;
use uikit::input::Input;
use uikit::layout::{FixedAdvance, Measure};
use uikit::widget::{UiError, UiEvent, Value};
use uikit::{ActError, Frame, Ui};
use wasmi::{Caller, Error, Extern};

/// The interface whose functions this module serves.
pub(crate) const VIEW: &str = "kai:ui/view";
/// The export events go to.
pub(crate) const INPUT: &str = "kai:ui/input";

/// Most updates kept for comphost; a window's frames coalesce, so this is a
/// window's worth plus its close.
pub const MAX_UPDATES: usize = 2 * uikit::MAX_WINDOWS;

/// What comphost shows next.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// Everything committed since the window's last update, as one frame.
    Frame { window: u32, frame: Frame },
    /// The app closed the window.
    Closed(u32),
}

/// An instance's windows and the updates comphost has not taken.
#[derive(Default)]
pub(crate) struct UiHost {
    pub(crate) ui: Ui,
    pub(crate) updates: Vec<Update>,
    measure: Option<Box<dyn Measure>>,
}

/// The measure the windows are laid out with.
fn measure(m: &Option<Box<dyn Measure>>) -> &dyn Measure {
    m.as_deref().unwrap_or(&FixedAdvance)
}

impl UiHost {
    /// Keeps `frame`, merged into the window's pending frame if there is one: its
    /// damage joins the earlier damage, as one box past `MAX_DAMAGE`.
    fn shown(&mut self, window: u32, mut frame: Frame) {
        let pending = self.updates.iter_mut().find_map(|u| match u {
            Update::Frame { window: w, frame } if *w == window => Some(frame),
            _ => None,
        });
        if let Some(old) = pending {
            let mut damage = core::mem::take(&mut old.damage);
            damage.append(&mut frame.damage);
            frame.damage = merged(damage, frame.size);
            *old = frame;
            return;
        }
        self.updates.push(Update::Frame { window, frame });
    }

    fn closed(&mut self, window: u32) {
        self.updates
            .retain(|u| !matches!(u, Update::Frame { window: w, .. } if *w == window));
        self.updates.push(Update::Closed(window));
    }
}

/// `damage` cut to the window, as one box when there are too many rectangles.
fn merged(damage: Vec<Rect>, (w, h): (i32, i32)) -> Vec<Rect> {
    let window = Rect::new(0, 0, w, h);
    let damage: Vec<Rect> = damage
        .iter()
        .map(|d| window.intersect(d))
        .filter(|d| !d.is_empty())
        .collect();
    if damage.len() <= uikit::window::MAX_DAMAGE {
        return damage;
    }
    let all = damage.iter().skip(1).fold(damage[0], |a, d| a.union(d));
    vec![all]
}

/// The guest seen from inside a host function: its memory, for lifting the
/// arguments and storing the result.
struct InCall<'a, 'b> {
    caller: &'a mut Caller<'b, HostState>,
}

impl InCall<'_, '_> {
    fn memory(&self) -> Result<wasmi::Memory, Trap> {
        self.caller
            .get_export("memory")
            .and_then(Extern::into_memory)
            .ok_or_else(|| Trap("no memory".into()))
    }
}

impl Guest for InCall<'_, '_> {
    fn read(&mut self, at: u32, len: u32) -> Result<Vec<u8>, Trap> {
        let start = at as usize;
        let end = start
            .checked_add(len as usize)
            .ok_or_else(|| Trap("read past memory".into()))?;
        self.memory()?
            .data(&*self.caller)
            .get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Trap("read past memory".into()))
    }

    fn write(&mut self, at: u32, bytes: &[u8]) -> Result<(), Trap> {
        self.memory()?
            .write(&mut *self.caller, at as usize, bytes)
            .map_err(|_| Trap("write past memory".into()))
    }

    fn realloc(&mut self, _: u32, _: u32, _: u32, _: u32) -> Result<u32, Trap> {
        Err(Trap("kai:ui/view returns nothing that needs memory".into()))
    }

    fn call(&mut self, export: &str, _: &[Core]) -> Result<Vec<Core>, Trap> {
        Err(Trap(format!("no calls into the guest from {export}")))
    }

    fn rep(&mut self, _: u32) -> Result<u32, Trap> {
        Err(Trap("kai:ui has no resources".into()))
    }
}

fn core_of(v: &wasmi::Val) -> Result<Core, Error> {
    Ok(match v {
        wasmi::Val::I32(v) => Core::I32(*v),
        wasmi::Val::I64(v) => Core::I64(*v),
        wasmi::Val::F32(v) => Core::F32((*v).into()),
        wasmi::Val::F64(v) => Core::F64((*v).into()),
        _ => return Err(Error::new("a parameter outside the core types")),
    })
}

/// One `kai:ui/view` call: lifts the arguments, runs it on the instance's
/// `Ui`, and stores the result where the guest asked.
pub(crate) fn view(
    mut caller: Caller<'_, HostState>,
    entry: &'static WitFunc,
    params: &[wasmi::Val],
) -> Result<(), Error> {
    let core = params.iter().map(core_of).collect::<Result<Vec<_>, _>>()?;
    let func = entry.func(String::new());
    let mut guest = InCall {
        caller: &mut caller,
    };
    let (args, ret) = appabi::lift_args(&mut guest, &func, &core).map_err(|t| Error::new(t.0))?;
    let result = run(&mut guest.caller.data_mut().ui, entry.name, &args)
        .map_err(|why| Error::new(format!("{VIEW} {}: {why}", entry.name)))?;
    let value = match result {
        Ok(Some(id)) => Val::Result(Ok(Some(Box::new(Val::U32(id))))),
        Ok(None) => Val::Result(Ok(None)),
        Err(e) => Val::Result(Err(Some(Box::new(Val::Enum(e as u32))))),
    };
    let (Some(ty), Some(at)) = (func.result.as_ref(), ret) else {
        return Err(Error::new("a kai:ui/view result with nowhere to go"));
    };
    appabi::store_result(&mut guest, &value, ty, at).map_err(|t| Error::new(t.0))
}

/// The call itself: `Ok(Ok(id))` for `open`, `Ok(Ok(None))` for the rest, the
/// app's error, or why its arguments make no sense (a trap).
fn run(
    host: &mut UiHost,
    name: &str,
    args: &[Val],
) -> Result<Result<Option<u32>, UiError>, String> {
    if host.updates.len() >= MAX_UPDATES {
        return Err("too many window updates before comphost took them".into());
    }
    if name == "open" {
        let title = str_of(args.first())?;
        let (w, h) = (u16_of(args.get(1))?, u16_of(args.get(2))?);
        return Ok(host.ui.open(&title, w, h).map(Some));
    }
    let window = u32_of(args.first())?;
    let node = || u32_of(args.get(1));
    let done = match name {
        "set-title" => {
            let title = str_of(args.get(1))?;
            host.ui.window(window).map(|w| w.set_title(&title))
        }
        "put" => {
            let (node, parent) = (node()?, u32_of(args.get(2))?);
            let (layout, widget) = (layout(args.get(3))?, widget(args.get(4))?);
            host.ui
                .window(window)
                .and_then(|w| w.put(node, parent, layout, widget))
        }
        "remove" => {
            let node = node()?;
            host.ui.window(window).and_then(|w| w.remove(node))
        }
        "focus" => {
            let node = node()?;
            host.ui.window(window).and_then(|w| w.focus(node))
        }
        "commit" => {
            let m = measure(&host.measure);
            let frame = host.ui.window(window).map(|w| w.commit(m));
            frame.map(|frame| host.shown(window, frame))
        }
        "close" => host.ui.close(window).map(|()| host.closed(window)),
        "set-unsaved" => {
            let unsaved = bool_of(args.get(1))?;
            host.ui.window(window).map(|w| w.set_unsaved(unsaved))
        }
        other => return Err(format!("{other} is not a view function")),
    };
    Ok(done.map(|()| None))
}

/// Why the agent's action did not reach the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActRefused {
    NoWindow,
    Node(ActError),
}

impl CoreInstance {
    /// Lays the app's windows out with `measure` from now on: the fonts
    /// comphost draws with.
    pub fn set_measure(&mut self, measure: Box<dyn Measure>) {
        self.store.data_mut().ui.measure = Some(measure);
    }

    /// What comphost shows next, oldest first; taking them empties the list.
    pub fn take_ui(&mut self) -> Vec<Update> {
        core::mem::take(&mut self.store.data_mut().ui.updates)
    }

    /// The windows the app has open.
    pub fn ui_windows(&self) -> Vec<u32> {
        self.store.data().ui.ui.ids().collect()
    }

    /// Routes one input to a window and gives the app what it caused. Input for
    /// a window the app has closed is dropped.
    pub fn ui_input(&mut self, window: u32, input: Input) -> Result<(), Fault> {
        let host = &mut self.store.data_mut().ui;
        let m = measure(&host.measure);
        let events = host.ui.window(window).map(|w| w.input(input, m));
        let Ok(events) = events else {
            return Ok(());
        };
        self.ui_events(window, &events)
    }

    /// The agent's AccessKit action on a node, which reaches the app as the same
    /// events a user's input would.
    pub fn ui_act(
        &mut self,
        window: u32,
        node: u32,
        action: Action,
        value: Option<Value>,
    ) -> Result<Result<(), ActRefused>, Fault> {
        let host = &mut self.store.data_mut().ui;
        let m = measure(&host.measure);
        let acted = host
            .ui
            .window(window)
            .map(|w| w.act(node, action, value, m));
        let Ok(acted) = acted else {
            return Ok(Err(ActRefused::NoWindow));
        };
        match acted {
            Ok(events) => self.ui_events(window, &events).map(Ok),
            Err(e) => Ok(Err(ActRefused::Node(e))),
        }
    }

    /// Gives the app `events` from `window`, one `on-ui` call each. An app that
    /// does not export `kai:ui/input` gets none.
    pub fn ui_events(&mut self, window: u32, events: &[UiEvent]) -> Result<(), Fault> {
        let Some(export) = self.on_ui.clone() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == INPUT && f.name == "on-ui")
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:ui/input table"))?;
        let func = entry.func(export);
        for e in events {
            let args = [Val::U32(window), uival::event(window, e)];
            livecore::Instance::call(self, &func, &args)?;
        }
        Ok(())
    }
}
