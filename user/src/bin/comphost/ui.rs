//! An app's windows on `desktop` (ADR 0005 sections 3-4): what the shown
//! version's `kai:ui` calls committed, painted with `uipaint` into a pool per
//! window and sent over `kai-surface 1` (`wasmhost::desk` builds the requests),
//! and `desktop`'s close requests and the agent's actions handed back to the
//! app. Only a process whose plan line has `uses=desktop` has a route for it.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use deskcore::effects;
use gfx2d::Canvas;
use gfx2d::text::{Fonts, Text};
use kabi::handle::{Handle, Rights};
use livecore::host::{Env, Exit, Host};
use livecore::{Fault, Performed};
use uikit::Frame;
use uikit::layout::Measure;
use uikit::style::TypeStyle;
use uikit::widget::UiEvent;
use user::{println, sys};
use wasmhost::desk::{Desk, Open, Show, Target, Told};
use wasmhost::{AppEvent, LiveInstance, Update, surface};

/// How long a call to `desktop` may take.
const CALL_MS: u64 = 5_000;
/// Times one pump tells the app the size of its code views and looks again.
const SIZE_ROUNDS: usize = 4;
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
    /// Whether one of its windows took focus since [`Ui::take_focused`].
    focused: bool,
    /// Events that came back with the window's own calls, acted on at the next
    /// poll.
    later: Vec<Told>,
    reply: Vec<u8>,
    /// The tickets sent to `desktop` with no result yet: only then does a poll
    /// ask for results, and only theirs reach the app.
    sent: effects::Sent,
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
            focused: false,
            later: Vec::new(),
            reply: alloc::vec![0u8; kabi::MAX_MESSAGE],
            sent: effects::Sent::default(),
        })
    }

    pub fn has_windows(&self) -> bool {
        !self.frames.is_empty()
    }

    /// Whether comphost should poll: a window is open, or an effect's result
    /// is still to come (one queued as the last window closed, too).
    pub fn wants_poll(&self) -> bool {
        self.has_windows() || self.sent.awaits()
    }

    /// Whether one of its windows took focus since the last call: use, for
    /// app nap (P3 task 12b).
    pub fn take_focused(&mut self) -> bool {
        core::mem::take(&mut self.focused)
    }

    /// Shows what the instances committed since the last call: the shown
    /// version's frames and closes, and nothing of a standby's. A new version
    /// shown closes the old one's windows first. A suspended app's windows stay
    /// open with their last frame, so the user can restore one and a standing
    /// instruction's "until restore" can come (P3 task 11).
    pub fn pump<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>) {
        // A code view's size is told to the app in a call of its own, which may
        // change what it shows (a few rounds: the size settles at once, but an app
        // that changed its layout on every size would not).
        for _ in 0..SIZE_ROUNDS {
            let sized = self.pump_once(host);
            if sized.is_empty() {
                return;
            }
            for (window, node, rows, cells) in sized {
                let performed = serve_shown(host, |core| core.code_size(window, node, rows, cells));
                // What the app queued while it handled the size goes out as for a close.
                self.forward(performed);
            }
        }
    }

    /// One look at what the instances committed; the code views whose size the
    /// app is yet to be told.
    fn pump_once<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
    ) -> Vec<(u32, u32, u16, u16)> {
        let mut sized = Vec::new();
        if host.suspended() {
            return sized;
        }
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
                Update::CodeSize {
                    window,
                    node,
                    rows,
                    cells,
                } => sized.push((window, node, rows, cells)),
            }
        }
        sized
    }

    /// Asks `desktop` for its events and acts on them, then hands the app the
    /// input `desktop` posted to its windows' event rings.
    pub fn poll<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
    ) -> Result<(), Exit> {
        if self.has_windows() || !self.later.is_empty() {
            let mut told = core::mem::take(&mut self.later);
            if let Ok(len) = sys::call_route(self.route, &[], &mut self.reply, CALL_MS) {
                told.extend(self.desk.told(&self.reply[..len.min(self.reply.len())]));
            }
            self.act(host, told)?;
        }
        // A suspended app's input waits in its rings, and its effects' results
        // at `desktop`: serving either would wake the app, and what resumes it
        // (a rule's `until`, `ctl resume`) goes first.
        if host.suspended() {
            return Ok(());
        }
        let inputs = self.take_input();
        let typed = !inputs.is_empty();
        for (app, input) in inputs {
            // Typed input is a transaction too: what the app saves or asks for
            // because of it is kept like an action call's.
            let performed = serve_shown(host, |core| core.ui_input(app, input));
            self.forward(performed);
        }
        if typed {
            self.pump(host);
        }
        self.deliver(host);
        Ok(())
    }

    /// Sends what the app's transaction queued to `desktop`, for the shell to
    /// run; `desktop` knows which app this is from the kernel, not from us.
    fn forward(&mut self, performed: Option<Performed>) {
        let Some(performed) = performed.filter(|p| !p.is_empty()) else {
            return;
        };
        let lines = performed.effect_lines();
        let request = alloc::format!("{}{lines}", effects::PUT);
        match sys::call_route(self.route, request.as_bytes(), &mut self.reply, CALL_MS) {
            Ok(len) if &self.reply[..len.min(self.reply.len())] == b"ok" => {
                self.sent.record(&lines);
            }
            Ok(len) => println!(
                "comphost: desktop refused the effects: {}",
                String::from_utf8_lossy(&self.reply[..len.min(self.reply.len())])
            ),
            Err(error) => println!("comphost: desktop did not take the effects ({error:?})"),
        }
    }

    /// Gives the app what came of its effects, one `effect-done` each, for
    /// tickets it sent only; what it queues in answer goes out the same way.
    fn deliver<E: Env<Instance = LiveInstance>>(&mut self, host: &mut Host<E>) {
        if !self.sent.awaits() {
            return;
        }
        let Ok(len) = sys::call_route(
            self.route,
            effects::RESULTS.as_bytes(),
            &mut self.reply,
            CALL_MS,
        ) else {
            return;
        };
        let text = String::from_utf8_lossy(&self.reply[..len.min(self.reply.len())]).into_owned();
        let (accepted, stray) = self.sent.accept(effects::parse_results(&text));
        for ticket in stray {
            println!("comphost: dropped a result for ticket {ticket}, which it did not send");
        }
        for done in accepted {
            let ticket = done.ticket;
            let event = AppEvent::EffectDone {
                ticket,
                outcome: done.outcome,
            };
            match serve_shown(host, |core| core.deliver_event(&event)) {
                Some(performed) => self.forward(Some(performed)),
                None => println!("comphost: the result for ticket {ticket} did not reach the app"),
            }
        }
        self.pump(host);
    }

    /// The input waiting in each window's event ring, as `uikit` input: keys
    /// with their text, buttons and pointer moves (ADR 0005 §3).
    fn take_input(&mut self) -> Vec<(u32, uikit::input::Input)> {
        let mut out = Vec::new();
        for pool in &self.pools {
            if (pool.bytes as usize) < surfabi::header::PAGE {
                continue;
            }
            // SAFETY: the pool is mapped read-write at `address` for `bytes`,
            // at least a page; its first page is the header set up at `open`.
            let page = unsafe { &mut *(pool.address as *mut [u8; surfabi::header::PAGE]) };
            loop {
                match surfabi::header::take(page) {
                    Ok(Some(event)) => {
                        if let Some(input) = self.desk.seat_state.input(&event) {
                            out.push((pool.app, input));
                        }
                    }
                    // One it cannot read is skipped.
                    Err(()) => {}
                    Ok(None) => break,
                }
            }
        }
        out
    }

    /// Hands `desktop`'s events to the app. A close or an action on the kept
    /// window of a suspended app resumes it first, so it is not dropped.
    fn act<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
        told: Vec<Told>,
    ) -> Result<(), Exit> {
        for t in told {
            // A focus wakes only an app comphost napped itself (its caller
            // decides); a close or an action wakes any suspended app.
            if !matches!(t, Told::Restarted | Told::Focus(_))
                && let Some(reply) = host.wake()?
            {
                println!(
                    "comphost: resumed for a window event: {}",
                    String::from_utf8_lossy(&reply)
                );
            }
            match t {
                Told::Restarted => self.reopen(),
                Told::Focus(_) => self.focused = true,
                Told::Close(window) => {
                    let performed = serve_shown(host, |core| {
                        core.ui_events(window, &[UiEvent::CloseRequested])
                    });
                    self.forward(performed);
                }
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
                    let performed = serve_shown(host, |core| {
                        core.ui_act(window, node, action, value.clone())
                            .map(|_refused_or_not| ())
                    });
                    self.forward(performed);
                }
            }
        }
        self.pump(host);
        Ok(())
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
        // The header page first: `desktop` posts the window's input to its ring.
        let header = surfabi::header::new_page(open.pool_bytes);
        // SAFETY: the pool is mapped read-write at `address`, a page at least.
        unsafe {
            core::ptr::copy_nonoverlapping(header.as_ptr(), address as *mut u8, header.len())
        };
        self.paint(&pool, open.target, frame);
        // Write, so `desktop` can post input to the header page's ring.
        let rights = Rights::MAP | Rights::READ | Rights::WRITE | Rights::TRANSFER;
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

/// Runs `f` on the serving version's instance as one transaction, when it is a
/// `lifecycle 2` one: what the app does about a window event (storage writes
/// and effects) is committed with its checkpoint, like an action call's. The
/// effects it queued come back for whoever runs them; a version that faulted
/// was reloaded, and gives nothing.
fn serve_shown<E: Env<Instance = LiveInstance>>(
    host: &mut Host<E>,
    f: impl FnOnce(&mut wasmhost::CoreInstance) -> Result<(), Fault>,
) -> Option<Performed> {
    let served = host.serve_with(|instance| match instance {
        LiveInstance::V2(core) => f(core),
        _ => Ok(()),
    });
    match served {
        Ok(done) => done.map(|((), performed)| performed),
        Err(exit) => {
            println!("comphost: {}", exit.why);
            None
        }
    }
}

/// Boxes a measure for `CoreInstance::set_measure`.
pub fn measure() -> Option<Box<dyn Measure>> {
    FontMeasure::new().map(|m| Box::new(m) as Box<dyn Measure>)
}
