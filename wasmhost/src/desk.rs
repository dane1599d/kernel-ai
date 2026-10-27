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
use surfabi::proto::{Event, Interface, REGISTRY, Request};
use surfabi::{Format, Rect, wire};
use uikit::Frame;

/// The globals' names, in `surfabi::proto::GLOBALS`'s order.
const POOL: u32 = 1;
const SURFACE: u32 = 2;
const SEAT: u32 = 3;
/// Buffers a window draws in turn.
const BUFFERS: u32 = 2;

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
}

struct Window {
    /// The app's `uikit` window id.
    app: u32,
    pool: u32,
    surface: u32,
    toplevel: u32,
    a11y: u32,
    buffers: [u32; BUFFERS as usize],
    width: u32,
    height: u32,
    /// The buffer the last commit showed.
    front: usize,
}

impl Window {
    fn buffer_bytes(&self) -> u32 {
        self.width * self.height * 4
    }

    fn target(&self, n: usize) -> Target {
        Target {
            offset: PAGE as u32 + n as u32 * self.buffer_bytes(),
            width: self.width,
            height: self.height,
            stride: self.width * 4,
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
        }
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
        let (w, h) = (frame.size.0.max(1) as u32, frame.size.1.max(1) as u32);
        let damage = surface::damage(frame, surface::SCALE_ONE);
        let tree = surface::tree_update(frame).encode();
        if let Some(win) = self
            .window(app)
            .filter(|win| (win.width, win.height) == (w, h))
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
            push_tree(&mut requests, win.a11y, &tree);
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
            front: 0,
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
                width: w,
                height: h,
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
        let all = Rect { x: 0, y: 0, w, h };
        encode(&mut requests, win.surface, Request::DamageBuffer(all));
        encode(&mut requests, win.surface, Request::Commit);
        push_tree(&mut requests, win.a11y, &tree);
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

    /// What one reply's events ask for. Events for objects this connection no
    /// longer has are dropped.
    pub fn told(&mut self, mut reply: &[u8]) -> Vec<Told> {
        let mut out = Vec::new();
        let mut greeting = false;
        while let Ok((header, args, next)) = wire::split(reply) {
            reply = next;
            if header.object == REGISTRY {
                // A greeting's globals come together: the first greeting is
                // this connection's own, any later one another desktop's.
                if !greeting {
                    greeting = true;
                    if self.greeted {
                        out.push(Told::Restarted);
                    }
                    self.greeted = true;
                }
                continue;
            }
            if Some(header.object) == self.seat {
                if let Ok(Event::Modifiers(mask)) =
                    Event::decode(Interface::Seat, header.opcode, args)
                {
                    self.seat_state.modifiers(mask);
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
