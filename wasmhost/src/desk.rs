//! comphost's side of `kai-surface 1` (ADR 0005 section 3): each `uikit`
//! window an app opens becomes a surface, its toplevel and a pool of its own
//! with two buffers, drawn in turn. This module only builds the requests and
//! reads the replies; comphost makes and maps the pools, paints, and makes the
//! calls. As `desktop` takes them until ADR 0007's doorbells: a call carries a
//! run of messages, a `pool.create` brings its `Shm` as the call's one handle,
//! and each reply carries the events waiting.

use crate::surface;
use alloc::string::String;
use alloc::vec::Vec;
use surfabi::header::PAGE;
use surfabi::proto::{Event, Interface, REGISTRY, Request, SCALE_MAX, SCALE_ONE};
use surfabi::{Format, Rect, wire};
use uikit::Frame;

/// The globals' names, in `surfabi::proto::GLOBALS`'s order.
const POOL: u32 = 1;
const SURFACE: u32 = 2;
const SEAT: u32 = 3;
/// Buffers a window draws in turn.
const BUFFERS: u32 = 2;
/// The most px a buffer has on a side that `desktop` takes: deskcore's
/// `server::MAX_BUFFER_PX`, which `desk_tests` checks this against.
pub(crate) const MAX_BUFFER_PX: u32 = 16_384;
/// The widest or tallest window an app may have, in logical px: at the
/// largest scale its buffers are still within [`MAX_BUFFER_PX`] a side, and
/// its pool's size within a u32 (bug hunt 2, H2-11).
pub const MAX_WINDOW_PX: u16 = (MAX_BUFFER_PX * SCALE_ONE / SCALE_MAX) as u16;

/// Where to draw a window's next picture: an offset into its pool, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

/// A window to open: a new pool of `pool_bytes` to create and map, the picture
/// to paint into it, then `calls` to make, the first with the pool's `Shm`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub pool_bytes: u32,
    pub target: Target,
    pub calls: Vec<Vec<u8>>,
}

/// A new picture for an open window: paint, then make `calls`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Show {
    pub target: Target,
    /// What changed, in buffer px.
    pub damage: Vec<Rect>,
    pub calls: Vec<Vec<u8>>,
}

/// What `desktop`'s events ask of comphost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Told {
    /// The desktop's whole appearance (`kai-appearance 1` text): the theme
    /// with its colour overrides, text size and legible type. Replaces the
    /// `Theme` the same reply carries.
    Look(String),
    /// The user asked to close the app's `window`.
    Close(u32),
    /// The agent asked a node of `window` to act.
    Action {
        window: u32,
        node: u32,
        action: u32,
        value: String,
    },
    /// A second `desktop` greeted the connection: every window must open again.
    Restarted,
    /// One of the app's windows took the keyboard focus (`seat.focus_in`): a
    /// click on it, or Alt+Tab to it.
    Focus(u32),
    /// The keyboard focus left one of the app's windows (`seat.focus_out`):
    /// it is no longer in use, so the app may nap once unused.
    Blur(u32),
    /// The desktop's theme, by name (`surfabi`'s `Theme`): at connect and
    /// after each change. The app's windows paint in it; a name the app does
    /// not know leaves them as they are.
    Theme(String),
    /// The scale `desktop` prefers changed (`registry.scale`) while windows
    /// were open and no new `desktop` came: every window is drawn again at it.
    Scale,
}

struct Window {
    /// The app's `uikit` window id.
    app: u32,
    pool: u32,
    surface: u32,
    toplevel: u32,
    a11y: u32,
    buffers: [u32; BUFFERS as usize],
    /// The window's size in logical px; its buffers are this at `scale`.
    width: u32,
    height: u32,
    /// The scale its buffers are drawn at, in 120ths.
    scale: u32,
    /// The buffer the last commit showed.
    front: usize,
    /// The declared actions `desktop` was last told its nodes run, by wire id.
    bound: Vec<(u32, String)>,
    /// What `desktop` was last told of its unsaved changes.
    unsaved: bool,
    /// Whether `desktop` was last told it is kept from the agent.
    agent_private: bool,
}

/// `logical` px at `scale` in buffer px, rounded to the nearest as
/// `uipaint::Scale` rounds an edge at 0.
fn buffer_px(logical: u32, scale: u32) -> u32 {
    (logical * scale + SCALE_ONE / 2) / SCALE_ONE
}

impl Window {
    /// The buffers' size in px.
    fn buffer_size(&self) -> (u32, u32) {
        (
            buffer_px(self.width, self.scale),
            buffer_px(self.height, self.scale),
        )
    }

    fn buffer_bytes(&self) -> u32 {
        let (w, h) = self.buffer_size();
        w * h * 4
    }

    fn target(&self, n: usize) -> Target {
        let (width, height) = self.buffer_size();
        Target {
            offset: PAGE as u32 + n as u32 * self.buffer_bytes(),
            width,
            height,
            stride: width * 4,
        }
    }
}

/// One connection's windows.
pub struct Desk {
    next_id: u32,
    seat: Option<u32>,
    windows: Vec<Window>,
    /// The first `desktop`'s globals came: any more are another's.
    greeted: bool,
    /// `seat.modifiers`, for the windows' input.
    pub seat_state: surface::Seat,
    /// The scale `desktop` prefers, in 120ths, as `registry.scale` last said
    /// (1x until it does): new windows draw at it.
    scale: u32,
}

impl Default for Desk {
    fn default() -> Self {
        Self::new()
    }
}

fn encode(out: &mut Vec<u8>, object: u32, request: Request<'_>) {
    // Every request here has arguments the encoder takes.
    if let Ok(bytes) = request.encode(object) {
        out.extend_from_slice(&bytes);
    }
}

impl Desk {
    pub fn new() -> Self {
        Self {
            next_id: REGISTRY + 1,
            seat: None,
            windows: Vec::new(),
            greeted: false,
            seat_state: surface::Seat::default(),
            scale: SCALE_ONE,
        }
    }

    /// The scale windows draw at, in 120ths.
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// Whether `desktop`'s greeting has come: until it has, the scale is not
    /// known.
    pub fn greeted(&self) -> bool {
        self.greeted
    }

    fn id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The app's windows open on this connection.
    pub fn windows(&self) -> Vec<u32> {
        self.windows.iter().map(|w| w.app).collect()
    }

    fn window(&mut self, app: u32) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.app == app)
    }

    /// The first picture of the app's window `app`, or a new picture of it.
    pub fn frame(&mut self, app: u32, frame: &Frame) -> Result<Show, Open> {
        // Bounded as `view.open` bounds it: a size past it (a resize, or a
        // window opened before the bound) is drawn cut to it, never
        // overflowing the buffers' arithmetic.
        let side = |px: i32| px.clamp(1, i32::from(MAX_WINDOW_PX)) as u32;
        let (w, h) = (side(frame.size.0), side(frame.size.1));
        let scale = self.scale;
        let damage = surface::damage(frame, self.scale);
        let tree = surface::tree_update(frame).encode();
        if let Some(win) = self
            .window(app)
            .filter(|win| (win.width, win.height, win.scale) == (w, h, scale))
        {
            let back = 1 - win.front;
            win.front = back;
            let mut requests = Vec::new();
            encode(
                &mut requests,
                win.surface,
                Request::Attach {
                    buffer: win.buffers[back],
                },
            );
            for d in &damage {
                encode(&mut requests, win.surface, Request::DamageBuffer(*d));
            }
            encode(&mut requests, win.surface, Request::Commit);
            if let Some(title) = (!frame.title.is_empty()).then_some(frame.title.as_str()) {
                encode(&mut requests, win.toplevel, Request::SetTitle(title));
            }
            push_private(&mut requests, win, frame);
            push_tree(&mut requests, win.a11y, &tree);
            push_state(&mut requests, win, frame);
            return Ok(Show {
                target: win.target(back),
                damage,
                calls: calls(&requests),
            });
        }
        // New, or a new size: a window of its own from the start.
        let mut requests = self.forget(app);
        let seat = self.seat.is_none().then(|| self.id());
        if let Some(seat) = seat {
            encode(&mut requests, REGISTRY, bind(SEAT, "seat", seat));
            self.seat = Some(seat);
        }
        let win = Window {
            app,
            pool: self.id(),
            surface: self.id(),
            toplevel: self.id(),
            a11y: self.id(),
            buffers: [self.id(), self.id()],
            width: w,
            height: h,
            scale,
            front: 0,
            bound: Vec::new(),
            unsaved: false,
            agent_private: false,
        };
        let pool_bytes = PAGE as u32 + BUFFERS * win.buffer_bytes();
        encode(&mut requests, REGISTRY, bind(POOL, "pool", win.pool));
        encode(
            &mut requests,
            win.pool,
            Request::PoolCreate { size: pool_bytes },
        );
        encode(
            &mut requests,
            REGISTRY,
            bind(SURFACE, "surface", win.surface),
        );
        for (n, &buffer) in win.buffers.iter().enumerate() {
            let t = win.target(n);
            let made = Request::PoolBuffer {
                id: buffer,
                offset: t.offset,
                width: t.width,
                height: t.height,
                stride: t.stride,
                format: Format::Xrgb8888,
            };
            encode(&mut requests, win.pool, made);
        }
        encode(
            &mut requests,
            win.surface,
            Request::GetToplevel { id: win.toplevel },
        );
        encode(&mut requests, win.toplevel, Request::SetTitle(&frame.title));
        encode(
            &mut requests,
            win.surface,
            Request::GetA11y { id: win.a11y },
        );
        encode(
            &mut requests,
            win.surface,
            Request::Attach {
                buffer: win.buffers[0],
            },
        );
        let (bw, bh) = win.buffer_size();
        let all = Rect {
            x: 0,
            y: 0,
            w: bw,
            h: bh,
        };
        // Before the first buffer: it is drawn at the desktop's scale.
        if scale != SCALE_ONE {
            encode(&mut requests, win.surface, Request::SetBufferScale(scale));
        }
        encode(&mut requests, win.surface, Request::DamageBuffer(all));
        encode(&mut requests, win.surface, Request::Commit);
        let mut win = win;
        push_private(&mut requests, &mut win, frame);
        push_tree(&mut requests, win.a11y, &tree);
        push_state(&mut requests, &mut win, frame);
        let target = win.target(0);
        self.windows.push(win);
        Err(Open {
            pool_bytes,
            target,
            calls: calls(&requests),
        })
    }

    /// The app closed `app`: the requests that end its window, if it had one.
    pub fn closed(&mut self, app: u32) -> Vec<u8> {
        self.forget(app)
    }

    fn forget(&mut self, app: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let Some(at) = self.windows.iter().position(|w| w.app == app) else {
            return out;
        };
        let win = self.windows.remove(at);
        encode(&mut out, win.toplevel, Request::ToplevelDestroy);
        for buffer in win.buffers {
            encode(&mut out, buffer, Request::BufferDestroy);
        }
        out
    }

    /// A new `desktop`: nothing of this connection's is left there.
    pub fn reset(&mut self) {
        self.windows.clear();
        self.seat = None;
        self.next_id = REGISTRY + 1;
    }

    /// Whether comphost must call `desktop` before it opens a window: no
    /// greeting has come, or nothing of this connection is open there, so
    /// comphost, which polls only while a window is open, would not have
    /// heard that `desktop` forgot the connection meanwhile (the console
    /// took the screen, it restarted, or `reap` dropped a hidden app's
    /// unread events).
    pub fn ask_before_open(&self) -> bool {
        !self.greeted || self.windows.is_empty()
    }

    /// The events in the reply to that call. A new desktop's greeting is
    /// acted on here, not told: nothing of this connection is open there, so
    /// this side starts again and the next window opens once, on the
    /// connection `desktop` has now. Left to the open's own reply, it made
    /// comphost reopen the window it had just opened, as a second one that
    /// could never close (H3-26). True in the second place when it started
    /// again.
    pub fn told_before_open(&mut self, reply: &[u8]) -> (Vec<Told>, bool) {
        let mut told = self.told(reply);
        let anew = told.contains(&Told::Restarted);
        if anew {
            told.retain(|t| *t != Told::Restarted);
            self.reset();
        }
        (told, anew)
    }

    /// What one reply's events ask for. Events for objects this connection no
    /// longer has are dropped.
    pub fn told(&mut self, mut reply: &[u8]) -> Vec<Told> {
        let mut out = Vec::new();
        let mut greeting = false;
        let (mut restarted, was) = (false, self.scale);
        while let Ok((header, args, next)) = wire::split(reply) {
            reply = next;
            // Only `Global` (opcode 0) is a greeting; the registry's other
            // events (the theme) are no new desktop.
            if header.object == REGISTRY && header.opcode != 0 {
                match Event::decode(Interface::Registry, header.opcode, args) {
                    Ok(Event::Theme(name)) => out.push(Told::Theme(String::from(name))),
                    Ok(Event::Look(text)) => out.push(Told::Look(String::from(text))),
                    // A scale no surface may ask for is not one to draw at.
                    Ok(Event::Scale(scale)) if (SCALE_ONE..=SCALE_MAX).contains(&scale) => {
                        self.scale = scale;
                    }
                    _ => {}
                }
                continue;
            }
            if header.object == REGISTRY {
                // A greeting's globals come together: the first greeting is
                // this connection's own, any later one another desktop's.
                if !greeting {
                    greeting = true;
                    if self.greeted {
                        out.push(Told::Restarted);
                        // A new desktop says its own scale, or none at 1x.
                        self.scale = SCALE_ONE;
                        restarted = true;
                    }
                    self.greeted = true;
                }
                continue;
            }
            if Some(header.object) == self.seat {
                match Event::decode(Interface::Seat, header.opcode, args) {
                    Ok(Event::Modifiers(mask)) => self.seat_state.modifiers(mask),
                    Ok(Event::FocusIn { surface }) => {
                        if let Some(w) = self.windows.iter().find(|w| w.surface == surface) {
                            out.push(Told::Focus(w.app));
                        }
                    }
                    Ok(Event::FocusOut { surface }) => {
                        if let Some(w) = self.windows.iter().find(|w| w.surface == surface) {
                            out.push(Told::Blur(w.app));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let by_toplevel = self.windows.iter().find(|w| w.toplevel == header.object);
            if let Some(w) = by_toplevel {
                if Event::decode(Interface::Toplevel, header.opcode, args) == Ok(Event::Close) {
                    out.push(Told::Close(w.app));
                }
                continue;
            }
            let by_a11y = self.windows.iter().find(|w| w.a11y == header.object);
            if let Some(w) = by_a11y
                && let Ok(Event::ActionRequest {
                    node,
                    action,
                    value,
                }) = Event::decode(Interface::A11y, header.opcode, args)
            {
                out.push(Told::Action {
                    window: w.app,
                    node,
                    action,
                    value: String::from(value),
                });
            }
        }
        if self.scale != was && !restarted && !self.windows.is_empty() {
            out.push(Told::Scale);
        }
        // The whole appearance says more than the theme's name beside it.
        if out.iter().any(|t| matches!(t, Told::Look(_))) {
            out.retain(|t| !matches!(t, Told::Theme(_)));
        }
        out
    }
}

/// `requests` cut into calls of at most `kabi::MAX_MESSAGE` bytes, at message
/// boundaries and in order.
pub fn calls(mut requests: &[u8]) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    while let Ok((header, _, rest)) = wire::split(requests) {
        let message = &requests[..usize::from(header.size)];
        match out.last_mut() {
            Some(call) if call.len() + message.len() <= kabi::MAX_MESSAGE => {
                call.extend_from_slice(message)
            }
            _ => out.push(message.to_vec()),
        }
        requests = rest;
    }
    out
}

fn bind(name: u32, interface: &str, id: u32) -> Request<'_> {
    Request::Bind {
        name,
        interface,
        version: 1,
        id,
    }
}

/// `toplevel.set_agent_private` when it changed: before the tree, so `desktop`
/// never holds a private window's contents as shared.
fn push_private(out: &mut Vec<u8>, win: &mut Window, frame: &Frame) {
    if frame.agent_private != win.agent_private {
        encode(
            out,
            win.toplevel,
            Request::SetAgentPrivate(frame.agent_private),
        );
        win.agent_private = frame.agent_private;
    }
}

/// What changed of the frame's bound actions and unsaved changes since
/// `desktop` was last told: `a11y.bind_action` for each node bound anew, to
/// another action or no longer (an empty name), then `toplevel.set_unsaved`.
fn push_state(out: &mut Vec<u8>, win: &mut Window, frame: &Frame) {
    let bound: Vec<(u32, String)> = frame
        .a11y
        .iter()
        .filter_map(|n| Some((surface::wire_id(n.id), n.bound.clone()?)))
        .collect();
    for (node, _) in win
        .bound
        .iter()
        .filter(|(n, _)| !bound.iter().any(|(m, _)| m == n))
    {
        let unbind = Request::BindAction {
            node: *node,
            action: "",
        };
        encode(out, win.a11y, unbind);
    }
    for (node, action) in bound.iter().filter(|b| !win.bound.contains(b)) {
        let bind = Request::BindAction {
            node: *node,
            action,
        };
        encode(out, win.a11y, bind);
    }
    win.bound = bound;
    if frame.unsaved != win.unsaved {
        encode(out, win.toplevel, Request::SetUnsaved(frame.unsaved));
        win.unsaved = frame.unsaved;
    }
}

/// The tree as one `tree_update` and its commit, when it fits one message;
/// a larger tree waits for the doorbells' shared pages.
fn push_tree(out: &mut Vec<u8>, a11y: u32, tree: &[u8]) {
    let Ok(update) = Request::TreeUpdate(tree).encode(a11y) else {
        return;
    };
    if update.len() > surfabi::MAX_MESSAGE {
        return;
    }
    out.extend_from_slice(&update);
    encode(out, a11y, Request::A11yCommit);
}
