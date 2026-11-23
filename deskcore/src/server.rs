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
use surfabi::proto::{Event, GLOBALS, Objects, REGISTRY, Request, SCALE_ONE};
use surfabi::tree::{self, Tree};
use surfabi::wire::{self, Error};

/// The most encoded events a client may have waiting: 256 KiB, a few thousand
/// messages, far more than one round of a client that reads them.
pub const MAX_EVENT_BYTES: usize = 256 * 1024;

/// The largest coordinate or size of a damage rectangle kept, in px: far past any
/// buffer, and small enough that no sum of two overflows.
const MAX_DAMAGE_PX: u32 = 1 << 20;

/// The most px a buffer may have on a side: 16K, past any screen and any window
/// the desk keeps.
pub const MAX_BUFFER_PX: u32 = 16_384;

/// The most detents one wheel event carries to a client.
pub const MAX_DETENTS: i32 = 120;

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
    /// Whether it is kept from the agent (`toplevel.set_agent_private`).
    agent_private: bool,
    /// The scale its buffers are drawn at (`surface.set_buffer_scale`), in
    /// 120ths; 0 until it says, which is [`SCALE_ONE`].
    scale: u32,
}

impl Surface {
    fn buffer_scale(&self) -> u32 {
        match self.scale {
            0 => SCALE_ONE,
            s => s,
        }
    }
}

/// `buffer` px at `scale` in logical px, rounded to the nearest, as a
/// client rounds its logical size up to a buffer's.
fn logical(buffer: i32, scale: u32) -> i32 {
    ((i64::from(buffer) * i64::from(SCALE_ONE) + i64::from(scale) / 2) / i64::from(scale)) as i32
}

/// A rectangle of buffer px at `scale` as the logical one it lies in: grown
/// outward to whole logical px.
fn logical_damage(r: Rect, scale: u32) -> Rect {
    let (s, one) = (i64::from(scale), i64::from(SCALE_ONE));
    let lo = |v: i32| (i64::from(v) * one).div_euclid(s) as i32;
    let hi = |v: i32| ((i64::from(v) * one + s - 1).div_euclid(s)) as i32;
    let (x, y) = (lo(r.x), lo(r.y));
    Rect::new(x, y, hi(r.right()) - x, hi(r.bottom()) - y)
}

/// What `desktop` must do for a request beyond the desk itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Map the `Shm` the call brought (its first handle) as pool `pool`, of
    /// `size` bytes.
    MapPool { pool: u32, size: u32 },
    /// `window` shows `buffer` from now on, drawn at `scale` (120ths).
    Show {
        window: WindowId,
        buffer: Buffer,
        scale: u32,
    },
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
    /// Caps Lock, as the desk last said ([`Effect::Locks`]).
    caps: bool,
    /// Its surface that has the keyboard focus, if one does: when the focus
    /// leaves it, the client is told (`focus_out`), so its host knows the
    /// window is no longer in use (the app nap).
    focused: Option<u32>,
    /// Focus moves its own requests made (a window opened or closed), for
    /// every other client too ([`Client::take_shared`]): the window that lost
    /// the focus hears `focus_out`.
    shared: Vec<Effect>,
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
            caps: false,
            focused: None,
            shared: Vec::new(),
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

    /// Tells the client the scale the desktop prefers it to draw at, in
    /// 120ths: at connect, and only when it is not 1x, so a desktop at 1x
    /// sends the bytes it always did.
    pub fn scale(&mut self, scale: u32) {
        if scale != SCALE_ONE {
            self.send(REGISTRY, Event::Scale(scale));
        }
    }

    /// Tells the client the desktop's theme, by name: at connect and after a
    /// change (`surfabi`'s `Theme`).
    pub fn theme(&mut self, name: &str) {
        self.send(REGISTRY, Event::Theme(name));
    }

    /// Tells the client the desktop's whole appearance (`kai-appearance 1`
    /// text), so it paints and lays out as the desktop does (`surfabi`'s `Look`).
    pub fn look(&mut self, appearance: &str) {
        self.send(REGISTRY, Event::Look(appearance));
    }

    fn send(&mut self, object: u32, event: Event) {
        // Past the cap nothing more is kept: the connection is to be dropped
        // ([`Client::too_far_behind`]), so no queue grows without end.
        if self.events.len() > MAX_EVENT_BYTES {
            return;
        }
        if let Ok(bytes) = event.encode(object) {
            self.events.extend_from_slice(&bytes);
        }
    }

    /// Whether the client lets its events pile past [`MAX_EVENT_BYTES`]: it
    /// never answers, or answers slower than the desk speaks. `desktop` drops it.
    pub fn too_far_behind(&self) -> bool {
        self.events.len() > MAX_EVENT_BYTES
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

    /// Whether any buffer of the client is in pool `pool`.
    pub fn pool_in_use(&self, pool: u32) -> bool {
        self.buffers.iter().any(|b| b.1.pool == pool)
    }

    /// `desktop` let pool `pool` go: it is no pool of the client's any more, and
    /// a buffer made on it, or a commit of one, is refused.
    pub fn pool_gone(&mut self, pool: u32) {
        self.pools.retain(|p| p.0 != pool);
        self.buffers.retain(|b| b.1.pool != pool);
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
            } => (window, Kind::Key, usage, i32::from(down), None),
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
            // The vertical wheel: code 0, detents positive down, as many as
            // the person turned it by, within what a client can take in.
            Effect::Wheel {
                window,
                at,
                detents,
            } => (
                window,
                Kind::Wheel,
                0,
                detents.clamp(-MAX_DETENTS, MAX_DETENTS),
                Some(at),
            ),
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
            && let Some(ch) = crate::keymap::char_of(code, self.mods & 1 != 0, self.caps)
        {
            len = ch.encode_utf8(&mut text).len() as u8;
        }
        // A key has no place: it carries the modifiers held with it, its own change
        // included, so a client need not match the seat's message to the ring.
        let (x, y) = match kind {
            Kind::Key => ((self.mods as i32) << 8, 0),
            _ => at.map_or((0, 0), |p| (p.x << 8, p.y << 8)),
        };
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

    /// The modifiers the desk says are held (shift 1, ctrl 2, alt 4, super 8),
    /// whatever had the focus when they changed: the seat hears the new set.
    fn modifiers(&mut self, mods: u32) {
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
                    // A buffer has px, and no more per side than any screen: past that the
                    // layer a window's repaint allocates, or its row arithmetic, is no
                    // longer the client's to size.
                    let sides = 1..=MAX_BUFFER_PX;
                    if !sides.contains(&width) || !sides.contains(&height) {
                        return Err(Error::BadArg);
                    }
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
                Request::SetBufferScale(scale) => self.surface(header.object)?.scale = scale,
                Request::DamageBuffer(r) => {
                    // Whatever the client wrote: kept to what no sum of it can
                    // overflow (a damage past the buffer is cut where it is used).
                    let at = |v: u32| v.min(MAX_DAMAGE_PX) as i32;
                    let rect = Rect::new(at(r.x), at(r.y), at(r.w), at(r.h));
                    let surface = self.surface(header.object)?;
                    surface.damage.push(rect);
                    // A client that never commits cannot grow the list: past
                    // what a header page holds it is one rectangle, the bounding box.
                    if surface.damage.len() > surfabi::header::MAX_DAMAGE {
                        let all = surface
                            .damage
                            .iter()
                            .fold(Rect::new(0, 0, 0, 0), |all, r| all.union(r));
                        surface.damage = alloc::vec![all];
                    }
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
                Request::SetAgentPrivate(private) => {
                    self.surface_of_toplevel(header.object)?.agent_private = private;
                }
                Request::ToplevelDestroy => {
                    let surface = self.surface_of_toplevel(header.object)?;
                    surface.toplevel = None;
                    if let Some(window) = surface.window.take() {
                        let effects = desk.close(window);
                        needs.push(Need::Closed { window });
                        self.apply(&effects);
                        self.share(&effects);
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

    /// Whether `window`'s app keeps it from the agent: its `ui_view` is the
    /// title and "private" alone, and `ui_act` on it, focus included, is
    /// refused. comphost asks only for an app the system image grants it
    /// (Settings), so the flag is taken as it comes.
    pub fn agent_private(&self, window: WindowId) -> bool {
        self.surfaces
            .iter()
            .any(|s| s.window == Some(window) && s.agent_private)
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
        let surface = self
            .surfaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(Error::UnknownObject)?;
        // The window's size is the buffer's in logical px: at a buffer scale
        // above 1x, fewer of them than the buffer has.
        let scale = surface.buffer_scale();
        let size = Size {
            w: logical(buffer.width as i32, scale).max(1),
            h: logical(buffer.height as i32, scale).max(1),
        };
        let damage: Vec<Rect> = core::mem::take(&mut surface.damage)
            .into_iter()
            .map(|r| match scale {
                SCALE_ONE => r,
                s => logical_damage(r, s),
            })
            .collect();
        let window = match surface.window {
            Some(window) => {
                desk.commit(window, size, &damage);
                window
            }
            None => {
                // A window takes the keyboard only by the activation rule
                // (`Desk::may_activate`): not while the person is acting
                // elsewhere, so a background app cannot take their keys.
                let active = desk.may_activate(self.pid as u32);
                let (window, effects) =
                    desk.open(self.pid as u32, &label, &surface.title, size, active);
                desk.set_app_id(window, &surface.app_id.clone());
                surface.window = Some(window);
                self.apply(&effects);
                self.share(&effects);
                window
            }
        };
        Ok(Some(Need::Show {
            window,
            buffer,
            scale,
        }))
    }

    /// Turns the desk's effects that concern this client into events.
    pub fn apply(&mut self, effects: &[Effect]) {
        for effect in effects {
            self.input(effect);
            if let Effect::Locks { caps } = *effect {
                self.caps = caps;
            }
            if let Effect::Modifiers(mods) = *effect {
                self.modifiers(mods);
            }
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
                    let now = match focus {
                        Focus::Window(w) => self.surface_of_window(w),
                        _ => None,
                    };
                    if now == self.focused {
                        continue;
                    }
                    if let Some(surface) = self.focused {
                        self.send(seat, Event::FocusOut { surface });
                    }
                    if let Some(surface) = now {
                        self.send(seat, Event::FocusIn { surface });
                    }
                    self.focused = now;
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

    /// Keeps the focus moves among `effects` for every other client.
    fn share(&mut self, effects: &[Effect]) {
        self.shared.extend(
            effects
                .iter()
                .filter(|e| matches!(e, Effect::Focus(_)))
                .cloned(),
        );
    }

    /// The focus moves its last requests made, for `desktop` to give every
    /// other client ([`Client::apply`]), once.
    pub fn take_shared(&mut self) -> Vec<Effect> {
        core::mem::take(&mut self.shared)
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
