//! One client's `kai-surface 1` connection as `desktop` serves it: its objects,
//! pools, buffers and surfaces, turned into [`Desk`] calls and events back.
//! The bytes a client sends are decoded with `surfabi` and checked against the
//! connection's own objects; any error closes the connection, and nothing the
//! client sent before it has changed the desk.
//!
//! Until ADR 0007's doorbells, a request is an IPC call carrying a run of
//! messages, and the reply carries the events waiting for that client. A
//! `pool.create` brings its `Shm` as the call's first handle; `desktop` maps it
//! and tells [`Client::pool_mapped`] where.

use crate::desk::{Button, Desk, Effect, Focus, WindowId};
use crate::geom::{Rect, Size};
use alloc::string::String;
use alloc::vec::Vec;
use surfabi::Format;
use surfabi::header::Event as RingEvent;
use surfabi::input::{InputEvent, Kind};
use surfabi::proto::{Event, GLOBALS, Objects, REGISTRY, Request};
use surfabi::tree::{self, Tree};
use surfabi::wire::{self, Error};

/// A buffer in one of the client's pools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffer {
    pub pool: u32,
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: Format,
}

#[derive(Clone, Debug, Default)]
struct Surface {
    id: u32,
    toplevel: Option<u32>,
    window: Option<WindowId>,
    attached: Option<u32>,
    damage: Vec<Rect>,
    title: String,
    app_id: String,
    /// Its `a11y` object, and the tree the last `a11y.commit` applied.
    a11y: Option<u32>,
    tree: Tree,
    /// `tree_update` bytes waiting for their commit: one update may span
    /// several messages.
    pending: Vec<u8>,
    /// The declared action each bound node runs (`a11y.bind_action`), at most
    /// [`MAX_BOUND`].
    bound: Vec<(u32, String)>,
    /// Whether it holds changes not yet kept (`toplevel.set_unsaved`).
    unsaved: bool,
}

/// What `desktop` must do for a request beyond the desk itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Map the `Shm` the call brought (its first handle) as pool `pool`, of
    /// `size` bytes.
    MapPool { pool: u32, size: u32 },
    /// `window` shows `buffer` from now on.
    Show { window: WindowId, buffer: Buffer },
    /// `window` is gone: forget its buffer.
    Closed { window: WindowId },
}

/// An update's head, which [`tree::MAX_BYTES`] does not count.
const PENDING_HEAD: usize = 12;

/// Most nodes a window binds to declared actions.
pub const MAX_BOUND: usize = 256;
/// Longest action name: `<app>_<function>`, as an `agentabi` row's.
pub const MAX_ACTION: usize = 64;

pub struct Client {
    /// The client's process id, which also labels its windows.
    pub pid: u64,
    pub label: String,
    objects: Objects,
    /// Pools and their sizes, once mapped.
    pools: Vec<(u32, u32, bool)>,
    buffers: Vec<(u32, Buffer)>,
    surfaces: Vec<Surface>,
    seat: Option<u32>,
    /// Encoded events not yet sent.
    events: Vec<u8>,
    serial: u32,
    /// Modifier keys held, as `seat.modifiers` carries them: shift 1, ctrl 2,
    /// alt 4, super 8 (`uikit::input::Mods`).
    mods: u32,
    /// Input for its windows, by the pool whose header ring takes it.
    posts: Vec<(u32, RingEvent)>,
}

impl Client {
    /// A new connection: the registry announces the globals at once.
    pub fn new(pid: u64, label: &str) -> Self {
        let mut client = Self {
            pid,
            label: String::from(label),
            objects: Objects::new(),
            pools: Vec::new(),
            buffers: Vec::new(),
            surfaces: Vec::new(),
            seat: None,
            events: Vec::new(),
            serial: 0,
            mods: 0,
            posts: Vec::new(),
        };
        for (n, global) in GLOBALS.iter().enumerate() {
            client.send(
                REGISTRY,
                Event::Global {
                    name: n as u32 + 1,
                    interface: global.name(),
                    version: global.version(),
                },
            );
        }
        client
    }

    fn send(&mut self, object: u32, event: Event) {
        if let Ok(bytes) = event.encode(object) {
            self.events.extend_from_slice(&bytes);
        }
    }

    /// The events waiting for the client, as many whole messages as fit in
    /// `cap` bytes.
    pub fn take_events(&mut self, cap: usize) -> Vec<u8> {
        let mut end = 0;
        let mut rest = &self.events[..];
        while let Ok((header, _, next)) = wire::split(rest) {
            if end + usize::from(header.size) > cap {
                break;
            }
            end += usize::from(header.size);
            rest = next;
        }
        let out = self.events[..end].to_vec();
        self.events.drain(..end);
        out
    }

    /// The pool `pool` is mapped now.
    pub fn pool_mapped(&mut self, pool: u32) {
        if let Some(p) = self.pools.iter_mut().find(|p| p.0 == pool) {
            p.2 = true;
        }
    }

    pub fn buffer(&self, id: u32) -> Option<Buffer> {
        self.buffers.iter().find(|b| b.0 == id).map(|b| b.1)
    }

    /// The input for its windows since the last call, by pool: `desktop` posts
    /// each into that pool's header ring (ADR 0005 §3), where the client reads
    /// it without a message.
    pub fn take_posts(&mut self) -> Vec<(u32, RingEvent)> {
        core::mem::take(&mut self.posts)
    }

    /// The pool holding `window`'s buffer.
    fn pool_of(&self, window: WindowId) -> Option<u32> {
        let surface = self.surfaces.iter().find(|s| s.window == Some(window))?;
        self.buffer(surface.attached?).map(|b| b.pool)
    }

    /// An input effect for one of its windows, as a ring event: a key with the
    /// text it types (US layout, none while Ctrl, Alt or Super is held), a
    /// button, or the pointer's move; the modifiers go to the seat.
    fn input(&mut self, effect: &Effect) {
        let (window, kind, code, value, at) = match *effect {
            Effect::Key {
                window,
                usage,
                down,
            } => {
                self.modifier(usage, down);
                (window, Kind::Key, usage, i32::from(down), None)
            }
            Effect::Button {
                window,
                at,
                button,
                down,
            } => {
                let code = match button {
                    Button::Left => 1,
                    Button::Right => 2,
                    Button::Middle => 3,
                };
                (window, Kind::Button, code, i32::from(down), Some(at))
            }
            Effect::Pointer { window, at } => (window, Kind::Abs, 0, 0, Some(at)),
            _ => return,
        };
        let Some(pool) = self.pool_of(window) else {
            return;
        };
        let mut text = [0u8; 4];
        let mut len = 0;
        let command = self.mods & (2 | 4 | 8) != 0;
        if kind == Kind::Key
            && value == 1
            && !command
            && let Some(ch) = crate::keymap::char_of(code, self.mods & 1 != 0)
        {
            len = ch.encode_utf8(&mut text).len() as u8;
        }
        let (x, y) = at.map_or((0, 0), |p| (p.x << 8, p.y << 8));
        let event = RingEvent::Input {
            event: InputEvent {
                time_ns: 0,
                kind,
                code,
                value,
            },
            x,
            y,
            text,
            len,
        };
        self.posts.push((pool, event));
    }

    /// Shift, Ctrl, Alt and Super, held or let go: the seat hears the new set.
    fn modifier(&mut self, usage: u16, down: bool) {
        let bit = match usage {
            0xE1 | 0xE5 => 1,
            0xE0 | 0xE4 => 2,
            0xE2 | 0xE6 => 4,
            0xE3 | 0xE7 => 8,
            _ => return,
        };
        let mods = if down {
            self.mods | bit
        } else {
            self.mods & !bit
        };
        if mods == self.mods {
            return;
        }
        self.mods = mods;
        if let Some(seat) = self.seat {
            self.send(seat, Event::Modifiers(mods));
        }
    }

    /// The client's windows.
    pub fn windows(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.surfaces.iter().filter_map(|s| s.window)
    }

    /// Runs a call's messages against the desk. `has_handle` says whether the
    /// call brought a handle, which only `pool.create` may use.
    pub fn handle(
        &mut self,
        desk: &mut Desk,
        bytes: &[u8],
        has_handle: bool,
    ) -> Result<Vec<Need>, Error> {
        let mut needs = Vec::new();
        let mut rest = bytes;
        let mut handle_used = false;
        while !rest.is_empty() {
            let (header, args, next) = wire::split(rest)?;
            rest = next;
            let (_, request) = self.objects.receive(header.object, header.opcode, args)?;
            match request {
                Request::PoolCreate { size } => {
                    if !has_handle || handle_used || self.pools.iter().any(|p| p.0 == header.object)
                    {
                        return Err(Error::BadArg);
                    }
                    handle_used = true;
                    self.pools.push((header.object, size, false));
                    needs.push(Need::MapPool {
                        pool: header.object,
                        size,
                    });
                }
                Request::PoolBuffer {
                    id,
                    offset,
                    width,
                    height,
                    stride,
                    format,
                } => {
                    let pool = self
                        .pools
                        .iter()
                        .find(|p| p.0 == header.object)
                        .ok_or(Error::BadArg)?;
                    let end = u64::from(offset) + u64::from(stride) * u64::from(height);
                    if offset < surfabi::header::PAGE as u32 || end > u64::from(pool.1) {
                        return Err(Error::BadArg);
                    }
                    let buffer = Buffer {
                        pool: header.object,
                        offset,
                        width,
                        height,
                        stride,
                        format,
                    };
                    self.buffers.push((id, buffer));
                }
                Request::BufferDestroy => self.buffers.retain(|b| b.0 != header.object),
                Request::Bind {
                    id,
                    interface: "surface",
                    ..
                } => {
                    self.surfaces.push(Surface {
                        id,
                        ..Surface::default()
                    });
                }
                Request::Bind {
                    id,
                    interface: "seat",
                    ..
                } => self.seat = Some(id),
                Request::GetToplevel { id } => self.surface(header.object)?.toplevel = Some(id),
                Request::SetTitle(title) => {
                    let surface = self.surface_of_toplevel(header.object)?;
                    surface.title = String::from(title);
                    if let Some(window) = surface.window {
                        desk.set_title(window, title);
                    }
                }
                Request::SetAppId(app) => {
                    let surface = self.surface_of_toplevel(header.object)?;
                    surface.app_id = String::from(app);
                    if let Some(window) = surface.window {
                        desk.set_app_id(window, app);
                    }
                }
                Request::Attach { buffer } => self.surface(header.object)?.attached = Some(buffer),
                Request::DamageBuffer(r) => {
                    let rect = Rect::new(r.x as i32, r.y as i32, r.w as i32, r.h as i32);
                    self.surface(header.object)?.damage.push(rect);
                }
                Request::Commit => {
                    if let Some(need) = self.commit(desk, header.object)? {
                        needs.push(need);
                    }
                }
                Request::GetA11y { id } => self.surface(header.object)?.a11y = Some(id),
                Request::TreeUpdate(part) => {
                    let surface = self.surface_of_a11y(header.object)?;
                    if surface.pending.len() + part.len() > tree::MAX_BYTES + PENDING_HEAD {
                        return Err(Error::BadArg);
                    }
                    surface.pending.extend_from_slice(part);
                }
                // A bad tree closes the connection and the window keeps the
                // tree it had (ADR 0005 §3).
                Request::A11yCommit => {
                    let surface = self.surface_of_a11y(header.object)?;
                    let bytes = core::mem::take(&mut surface.pending);
                    let update = tree::Update::decode(&bytes).map_err(|_| Error::BadArg)?;
                    surface.tree.apply(update).map_err(|_| Error::BadArg)?;
                }
                Request::BindAction { node, action } => {
                    let surface = self.surface_of_a11y(header.object)?;
                    bind(&mut surface.bound, node, action)?;
                }
                Request::SetUnsaved(unsaved) => {
                    self.surface_of_toplevel(header.object)?.unsaved = unsaved;
                }
                Request::ToplevelDestroy => {
                    let surface = self.surface_of_toplevel(header.object)?;
                    surface.toplevel = None;
                    if let Some(window) = surface.window.take() {
                        let effects = desk.close(window);
                        needs.push(Need::Closed { window });
                        self.apply(&effects);
                    }
                }
                // The rest asks nothing of the desk yet (popups and the panel).
                _ => {}
            }
        }
        Ok(needs)
    }

    fn surface(&mut self, id: u32) -> Result<&mut Surface, Error> {
        self.surfaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(Error::UnknownObject)
    }

    fn surface_of_a11y(&mut self, a11y: u32) -> Result<&mut Surface, Error> {
        self.surfaces
            .iter_mut()
            .find(|s| s.a11y == Some(a11y))
            .ok_or(Error::UnknownObject)
    }

    /// `window`'s tree as its client last committed it: empty until then.
    pub fn tree(&self, window: WindowId) -> Option<&Tree> {
        self.surfaces
            .iter()
            .find(|s| s.window == Some(window))
            .map(|s| &s.tree)
    }

    /// The declared action `node` of `window` runs, so `ui_act` on it takes
    /// that action's tier; none for an unbound node (which asks), or one no
    /// longer in the tree.
    pub fn bound_action(&self, window: WindowId, node: u32) -> Option<&str> {
        let surface = self.surfaces.iter().find(|s| s.window == Some(window))?;
        surface.tree.node(node)?;
        surface
            .bound
            .iter()
            .find(|(n, _)| *n == node)
            .map(|(_, action)| action.as_str())
    }

    /// Whether `window`'s app says it holds changes not yet kept, so
    /// `window_close` asks first. A window of a client that never says is
    /// treated as saved, as ADR 0005 §6 has it for an app that reports nothing.
    pub fn unsaved(&self, window: WindowId) -> bool {
        self.surfaces
            .iter()
            .any(|s| s.window == Some(window) && s.unsaved)
    }

    /// Asks `window`'s app to run `action` (a bit number of
    /// [`surfabi::tree::action`]) on `node`, as an AccessKit `ActionRequest`:
    /// never synthetic input (ADR 0005 §6). False when the window has no tree
    /// or the node does not offer that action.
    pub fn act(&mut self, window: WindowId, node: u32, action: u32, value: &str) -> bool {
        let Some(surface) = self.surfaces.iter().find(|s| s.window == Some(window)) else {
            return false;
        };
        let offered = surface
            .tree
            .node(node)
            .is_some_and(|n| action < 16 && n.actions & (1 << action) != 0);
        let Some(a11y) = surface.a11y.filter(|_| offered) else {
            return false;
        };
        self.send(
            a11y,
            Event::ActionRequest {
                node,
                action,
                value,
            },
        );
        true
    }

    fn surface_of_toplevel(&mut self, toplevel: u32) -> Result<&mut Surface, Error> {
        self.surfaces
            .iter_mut()
            .find(|s| s.toplevel == Some(toplevel))
            .ok_or(Error::UnknownObject)
    }

    /// A surface's commit: a toplevel with a buffer opens its window the first
    /// time, and after that shows the new buffer and damage.
    fn commit(&mut self, desk: &mut Desk, id: u32) -> Result<Option<Need>, Error> {
        let label = self.label.clone();
        let surface = self
            .surfaces
            .iter()
            .find(|s| s.id == id)
            .ok_or(Error::UnknownObject)?;
        let (Some(_), Some(buffer_id)) = (surface.toplevel, surface.attached) else {
            return Ok(None);
        };
        let buffer = self.buffer(buffer_id).ok_or(Error::BadArg)?;
        // The pool may still be waiting to be mapped (a client may create it and
        // commit in one call): `desktop` carries out the needs in order, so it
        // maps the pool before it shows the buffer, and draws nothing from a
        // pool it could not map.
        if !self.pools.iter().any(|p| p.0 == buffer.pool) {
            return Err(Error::BadArg);
        }
        let size = Size {
            w: buffer.width as i32,
            h: buffer.height as i32,
        };
        let surface = self
            .surfaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(Error::UnknownObject)?;
        let damage = core::mem::take(&mut surface.damage);
        let window = match surface.window {
            Some(window) => {
                desk.commit(window, size, &damage);
                window
            }
            None => {
                // A window a client opens itself takes focus only through the
                // activation rule; for now every client's first window is the
                // user's (they started it).
                let (window, effects) =
                    desk.open(self.pid as u32, &label, &surface.title, size, true);
                desk.set_app_id(window, &surface.app_id.clone());
                surface.window = Some(window);
                self.apply(&effects);
                window
            }
        };
        Ok(Some(Need::Show { window, buffer }))
    }

    /// Turns the desk's effects that concern this client into events.
    pub fn apply(&mut self, effects: &[Effect]) {
        for effect in effects {
            self.input(effect);
            match *effect {
                Effect::Configure {
                    window,
                    size,
                    maximized,
                    activated,
                } => {
                    let Some(toplevel) = self.toplevel_of(window) else {
                        continue;
                    };
                    self.serial = self.serial.wrapping_add(1);
                    let mut states = Vec::new();
                    if maximized {
                        states.extend_from_slice(&1u32.to_le_bytes());
                    }
                    if activated {
                        states.extend_from_slice(&4u32.to_le_bytes());
                    }
                    let event = Event::Configure {
                        serial: self.serial,
                        width: size.w as u32,
                        height: size.h as u32,
                        states: &states,
                    };
                    if let Ok(bytes) = event.encode(toplevel) {
                        self.events.extend_from_slice(&bytes);
                    }
                }
                Effect::Close { window } => {
                    if let Some(toplevel) = self.toplevel_of(window) {
                        self.send(toplevel, Event::Close);
                    }
                }
                Effect::Focus(focus) => {
                    let Some(seat) = self.seat else { continue };
                    let mine = |w: WindowId| self.surface_of_window(w);
                    let event = match focus {
                        Focus::Window(w) => mine(w).map(|surface| Event::FocusIn { surface }),
                        _ => None,
                    };
                    if let Some(event) = event {
                        self.send(seat, event);
                    }
                }
                _ => {}
            }
        }
    }

    fn toplevel_of(&self, window: WindowId) -> Option<u32> {
        self.surfaces
            .iter()
            .find(|s| s.window == Some(window))?
            .toplevel
    }

    fn surface_of_window(&self, window: WindowId) -> Option<u32> {
        self.surfaces
            .iter()
            .find(|s| s.window == Some(window))
            .map(|s| s.id)
    }

    /// The pool a buffer lives in is gone or the client left: every window it
    /// opened closes.
    pub fn close_all(&mut self, desk: &mut Desk) -> Vec<Effect> {
        let mut effects = Vec::new();
        for window in self.windows().collect::<Vec<_>>() {
            effects.extend(desk.close(window));
        }
        self.surfaces.clear();
        effects
    }
}

/// Binds `node` to `action`, or unbinds it for an empty name. A name that is not
/// `<app>_<function>`-shaped, or a window past [`MAX_BOUND`], closes the
/// connection. Only the shape is checked here: which app's row it is, and so its
/// tier, the agent core resolves among the rows of this window's own app (the
/// kernel's pid, then that app's `kai-actions 1`), and a name not there asks.
fn bind(bound: &mut Vec<(u32, String)>, node: u32, action: &str) -> Result<(), Error> {
    bound.retain(|(n, _)| *n != node);
    if action.is_empty() {
        return Ok(());
    }
    let shaped = action.len() <= MAX_ACTION
        && action.contains('_')
        && !action.starts_with('_')
        && !action.ends_with('_')
        && action
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !shaped || bound.len() >= MAX_BOUND {
        return Err(Error::BadArg);
    }
    bound.push((node, String::from(action)));
    Ok(())
}
