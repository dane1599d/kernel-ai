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

use crate::desk::{Desk, Effect, Focus, WindowId};
use crate::geom::{Rect, Size};
use alloc::string::String;
use alloc::vec::Vec;
use surfabi::Format;
use surfabi::proto::{Event, GLOBALS, Objects, REGISTRY, Request};
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
                Request::ToplevelDestroy => {
                    let surface = self.surface_of_toplevel(header.object)?;
                    surface.toplevel = None;
                    if let Some(window) = surface.window.take() {
                        let effects = desk.close(window);
                        needs.push(Need::Closed { window });
                        self.apply(&effects);
                    }
                }
                // The rest asks nothing of the desk yet (popups, the tree and the
                // panel come with tasks 7 and 9).
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
