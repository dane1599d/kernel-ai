//! An app's windows on `desktop` (ADR 0005 sections 3-4): what the shown
//! version's `kai:ui` calls committed, painted with `uipaint` into a pool per
//! window and sent over `kai-surface 1` (`wasmhost::desk` builds the requests),
//! and `desktop`'s close requests and the agent's actions handed back to the
//! app. Only a process whose plan line has `uses=desktop` has a route for it.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::Canvas;
use gfx2d::text::{Fonts, Text};
use kabi::handle::{Handle, Rights};
use livecore::host::{Env, Host};
use uikit::Frame;
use uikit::layout::Measure;
use uikit::style::TypeStyle;
use uikit::widget::UiEvent;
use user::{println, sys};
use wasmhost::desk::{Desk, Open, Show, Target, Told};
use wasmhost::{LiveInstance, Update, surface};

/// How long a call to `desktop` may take.
const CALL_MS: u64 = 5_000;
/// How often an app with windows asks `desktop` for its events.
pub const POLL_MS: u64 = 20;

/// The fonts ADR 0005 §8 names, built in until P2's files carry them, as
/// `desktop`'s are: the one place a switch to `/system` fonts changes.
pub fn fonts() -> Fonts {
    Fonts {
        sans: include_bytes!("../../../../assets/fonts/InterVariable.ttf"),
        mono: include_bytes!("../../../../assets/fonts/JetBrainsMono-Variable.ttf"),
    }
}

/// `gfx2d`'s measure for an instance's layout: its own `Text`, so it lives as
/// long as the instance.
pub struct FontMeasure(Text);

impl FontMeasure {
    pub fn new() -> Option<Self> {
        Text::new(fonts(), look::DARK.dark).map(Self)
    }
}

impl Measure for FontMeasure {
    fn measure(&self, style: TypeStyle, text: &str) -> (f32, f32) {
        uipaint::Measured(&self.0).measure(style, text)
    }
}

/// A window's pool, mapped here.
struct Pool {
    app: u32,
    shm: Handle,
    address: u64,
    bytes: u32,
}

pub struct Ui {
    route: Handle,
    desk: Desk,
    text: Text,
    pools: Vec<Pool>,
    /// Each open window's last frame, to answer the agent and to reopen.
    frames: Vec<(u32, Frame)>,
    /// The version whose windows show.
    shown: Option<String>,
    /// Events that came back with the window's own calls, acted on at the next
    /// poll.
    later: Vec<Told>,
    reply: Vec<u8>,
}

impl Ui {
    /// The windows' side, when the plan gave a route to `desktop`.
    pub fn new() -> Option<Self> {
        let route = user::route(0)?;
        let Some(text) = Text::new(fonts(), look::DARK.dark) else {
            println!("comphost: the fonts do not parse; no windows");
            return None;
        };
        Some(Self {
            route,
            desk: Desk::new(),
            text,
            pools: Vec::new(),
            frames: Vec::new(),
            shown: None,
            later: Vec::new(),
            reply: alloc::vec![0u8; kabi::MAX_MESSAGE],
        })
    }

    pub fn has_windows(&self) -> bool {
        !self.frames.is_empty()
    }

    /// Shows what the instances committed since the last call: the shown
    /// version's frames and closes, and nothing of a standby's. A new version
    /// shown closes the old one's windows first.
    pub fn pump<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>) {
        let mut updates = Vec::new();
        let mut label = None;
        host.each_instance(|instance, version, shown| {
            if let LiveInstance::V2(core) = instance {
                let taken = core.take_ui();
                if shown {
                    updates = taken;
                    label = Some(String::from(version));
                }
            }
        });
        if label != self.shown {
            for (app, _) in core::mem::take(&mut self.frames) {
                self.close(app);
            }
            self.shown = label;
        }
        for update in updates {
            match update {
                Update::Frame { window, frame } => self.show(window, frame),
                Update::Closed(window) => {
                    self.frames.retain(|(app, _)| *app != window);
                    self.close(window);
                }
            }
        }
    }

    /// Asks `desktop` for its events and acts on them.
    pub fn poll<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>) {
        let mut told = core::mem::take(&mut self.later);
        if let Ok(len) = sys::call_route(self.route, &[], &mut self.reply, CALL_MS) {
            told.extend(self.desk.told(&self.reply[..len.min(self.reply.len())]));
        }
        self.act(host, told);
    }

    fn act<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>, told: Vec<Told>) {
        for t in told {
            match t {
                Told::Restarted => self.reopen(),
                Told::Close(window) => with_shown(host, |core| {
                    let _ = core.ui_events(window, &[UiEvent::CloseRequested]);
                }),
                Told::Action {
                    window,
                    node,
                    action,
                    value,
                } => {
                    let Some((_, frame)) = self.frames.iter().find(|(app, _)| *app == window)
                    else {
                        continue;
                    };
                    let Ok((node, action, value)) =
                        surface::action_request(frame, node, action, &value)
                    else {
                        continue;
                    };
                    with_shown(host, |core| {
                        let _ = core.ui_act(window, node, action, value.clone());
                    });
                }
            }
        }
        self.pump(host);
    }

    /// A new `desktop`: every window opens again from its last frame.
    fn reopen(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        for pool in core::mem::take(&mut self.pools) {
            release(&pool);
        }
        self.desk.reset();
        for (app, frame) in core::mem::take(&mut self.frames) {
            self.show(app, frame);
        }
        println!("comphost: reopened its windows after desktop restarted");
    }

    fn show(&mut self, app: u32, frame: Frame) {
        let title = frame.title.clone();
        let first = !self.frames.iter().any(|(a, _)| *a == app);
        match self.desk.frame(app, &frame) {
            Ok(show) => self.present(app, &frame, show),
            Err(open) => {
                if self.open(app, &frame, open) && first {
                    let (w, h) = frame.size;
                    println!("comphost: window {app} \"{title}\" shown at {w}x{h}");
                }
            }
        }
        self.frames.retain(|(a, _)| *a != app);
        self.frames.push((app, frame));
    }

    fn open(&mut self, app: u32, frame: &Frame, open: Open) -> bool {
        // A new size reopens: the old pool goes.
        if let Some(at) = self.pools.iter().position(|p| p.app == app) {
            release(&self.pools.remove(at));
        }
        let pages = u64::from(open.pool_bytes).div_ceil(4096);
        let Ok(shm) = sys::shm_create(pages) else {
            println!("comphost: no memory for window {app}");
            return false;
        };
        let Ok(address) = sys::shm_map(shm, true) else {
            let _ = sys::handle_close(shm);
            return false;
        };
        let pool = Pool {
            app,
            shm,
            address,
            bytes: open.pool_bytes,
        };
        self.paint(&pool, open.target, frame);
        let rights = Rights::MAP | Rights::READ | Rights::TRANSFER;
        let sent =
            sys::handle_dup(shm, rights).is_ok_and(|copy| self.send(&open.calls, Some(copy)));
        self.pools.push(pool);
        if !sent {
            println!("comphost: desktop refused window {app}");
        }
        sent
    }

    fn present(&mut self, app: u32, frame: &Frame, show: Show) {
        let Some(at) = self.pools.iter().position(|p| p.app == app) else {
            return;
        };
        let pool = self.pools.remove(at);
        self.paint(&pool, show.target, frame);
        self.pools.push(pool);
        self.send(&show.calls, None);
    }

    /// The whole picture into `target`: the other buffer holds an older one.
    fn paint(&mut self, pool: &Pool, target: Target, frame: &Frame) {
        let end = u64::from(target.offset) + u64::from(target.stride) * u64::from(target.height);
        if end > u64::from(pool.bytes) {
            return;
        }
        let px = (target.stride / 4) as usize * target.height as usize;
        // SAFETY: the pool is mapped read-write at `address` for `bytes`, and
        // the target lies inside it (checked above); `desktop` only reads the
        // buffer it was last given, which is the other one.
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(
                (pool.address + u64::from(target.offset)) as *mut u32,
                px,
            )
        };
        let mut canvas = Canvas {
            pixels,
            width: target.width,
            height: target.height,
            stride_px: target.stride / 4,
        };
        let all = canvas.bounds();
        let mut painter = uipaint::Painter {
            palette: &look::DARK,
            text: &mut self.text,
            scale: uipaint::Scale::of(uipaint::SCALE_ONE),
        };
        painter.paint(&mut canvas, &frame.list, all);
    }

    /// Makes `calls`, the first moving `handle`; the events in the replies are
    /// acted on at the next poll.
    fn send(&mut self, calls: &[Vec<u8>], mut handle: Option<Handle>) -> bool {
        let mut pending = Vec::new();
        for call in calls {
            let send: Vec<Handle> = handle.take().into_iter().collect();
            match sys::call_h(self.route, call, &mut self.reply, CALL_MS, &send, &mut []) {
                Ok((len, _)) => pending.extend_from_slice(&self.reply[..len.min(self.reply.len())]),
                Err(_) => return false,
            }
        }
        let told = self.desk.told(&pending);
        self.later.extend(told);
        true
    }

    fn close(&mut self, app: u32) {
        let calls = wasmhost::desk::calls(&self.desk.closed(app));
        if !calls.is_empty() {
            self.send(&calls, None);
            println!("comphost: window {app} closed");
        }
        if let Some(at) = self.pools.iter().position(|p| p.app == app) {
            release(&self.pools.remove(at));
        }
    }
}

fn release(pool: &Pool) {
    let _ = sys::shm_unmap(pool.address);
    let _ = sys::handle_close(pool.shm);
}

/// Runs `f` on the shown version's instance, when it is a `lifecycle 2` one.
fn with_shown<E: Env<Instance = LiveInstance>>(
    host: &mut Host<E>,
    mut f: impl FnMut(&mut wasmhost::CoreInstance),
) {
    host.each_instance(|instance, _, shown| {
        if let (LiveInstance::V2(core), true) = (instance, shown) {
            f(core);
        }
    });
}

/// Boxes a measure for `CoreInstance::set_measure`.
pub fn measure() -> Option<Box<dyn Measure>> {
    FontMeasure::new().map(|m| Box::new(m) as Box<dyn Measure>)
}
