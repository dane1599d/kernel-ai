//! The objects of `kai-surface 1` and their messages (ADR 0005 §3's table).
//!
//! A client starts with the registry, object 1. It binds the globals `pool`,
//! `surface`, `seat` and `panel` from it (a bind makes one new object; bind
//! `surface` again for another window). A pool makes buffers; a surface makes
//! the objects that give it a role, as Wayland's `xdg_surface` does: a
//! toplevel, a popup, its a11y tree, or, for system programs only, a panel.
//! `desktop` refuses `panel` to anyone else; that check is its own, not this
//! crate's.
//!
//! [`Objects`] is one connection's table of live ids. [`Objects::receive`]
//! decodes a request and applies what it does to the table (a bind or `get_*`
//! makes an object, `buffer.destroy` ends one), so an id that names nothing, or
//! a new id already taken, is an error before `desktop` acts on anything.

use crate::wire::{Error, Reader, Writer};
use crate::{Format, Rect};
use alloc::vec::Vec;

/// The registry's id on every connection.
pub const REGISTRY: u32 = 1;
/// Objects one connection may hold at once.
pub const MAX_OBJECTS: usize = 1024;
/// The highest id a client may choose (Wayland keeps the rest for the server).
pub const MAX_CLIENT_ID: u32 = 0xFEFF_FFFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interface {
    Registry,
    Pool,
    Buffer,
    Surface,
    Toplevel,
    Popup,
    Seat,
    A11y,
    Panel,
}

/// The globals a registry announces, in their `name` order (name = index + 1).
pub const GLOBALS: [Interface; 4] = [
    Interface::Pool,
    Interface::Surface,
    Interface::Seat,
    Interface::Panel,
];

impl Interface {
    pub const fn name(self) -> &'static str {
        match self {
            Interface::Registry => "registry",
            Interface::Pool => "pool",
            Interface::Buffer => "buffer",
            Interface::Surface => "surface",
            Interface::Toplevel => "toplevel",
            Interface::Popup => "popup",
            Interface::Seat => "seat",
            Interface::A11y => "a11y",
            Interface::Panel => "panel",
        }
    }

    /// Every object is at version 1 in `kai-surface 1`.
    pub const fn version(self) -> u32 {
        1
    }
}

/// A request from a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    // registry
    Bind {
        name: u32,
        interface: &'a str,
        version: u32,
        id: u32,
    },
    // pool: `create` comes by CALL_H with the `Shm` handle; `size` is the
    // `Shm`'s bytes, header page included.
    PoolCreate {
        size: u32,
    },
    PoolBuffer {
        id: u32,
        offset: u32,
        width: u32,
        height: u32,
        stride: u32,
        format: Format,
    },
    // buffer
    BufferDestroy,
    // surface
    Attach {
        buffer: u32,
    },
    DamageBuffer(Rect),
    Frame,
    SetOpaque(Rect),
    Commit,
    GetToplevel {
        id: u32,
    },
    GetPopup {
        id: u32,
        parent: u32,
    },
    GetA11y {
        id: u32,
    },
    GetPanel {
        id: u32,
    },
    // toplevel
    SetTitle(&'a str),
    SetAppId(&'a str),
    SetMinSize {
        width: u32,
        height: u32,
    },
    SetMaxSize {
        width: u32,
        height: u32,
    },
    AckConfigure {
        serial: u32,
    },
    /// The client closes its window (`xdg_toplevel.destroy`).
    ToplevelDestroy,
    // popup
    Position {
        anchor: u32,
        gravity: u32,
        x: i32,
        y: i32,
    },
    Grab {
        serial: u32,
    },
    // seat: a cursor-shape-v1 name such as "default" or "text"
    SetCursor(&'a str),
    // a11y: records of [`crate::tree`], applied at `A11yCommit`
    TreeUpdate(&'a [u8]),
    A11yCommit,
    BindAction {
        node: u32,
        action: u32,
    },
    // panel
    Layer(u32),
    ExclusiveZone(u32),
}

fn rect(r: &mut Reader) -> Result<Rect, Error> {
    Ok(Rect {
        x: r.size()?,
        y: r.size()?,
        w: r.size()?,
        h: r.size()?,
    })
}

impl<'a> Request<'a> {
    /// Decodes opcode `opcode` of `interface` from its argument bytes.
    pub fn decode(interface: Interface, opcode: u16, args: &'a [u8]) -> Result<Self, Error> {
        use Interface as I;
        let mut r = Reader::new(args);
        let request = match (interface, opcode) {
            (I::Registry, 0) => Request::Bind {
                name: r.uint()?,
                interface: r.string()?,
                version: r.uint()?,
                id: r.new_id()?,
            },
            (I::Pool, 0) => Request::PoolCreate { size: r.uint()? },
            (I::Pool, 1) => {
                let request = Request::PoolBuffer {
                    id: r.new_id()?,
                    offset: r.size()?,
                    width: r.size()?,
                    height: r.size()?,
                    stride: r.size()?,
                    format: Format::from_raw(r.uint()?).ok_or(Error::BadArg)?,
                };
                if !buffer_fits(&request) {
                    return Err(Error::BadArg);
                }
                request
            }
            (I::Buffer, 0) => Request::BufferDestroy,
            (I::Surface, 0) => Request::Attach {
                buffer: r.object()?,
            },
            (I::Surface, 1) => Request::DamageBuffer(rect(&mut r)?),
            (I::Surface, 2) => Request::Frame,
            (I::Surface, 3) => Request::SetOpaque(rect(&mut r)?),
            (I::Surface, 4) => Request::Commit,
            (I::Surface, 5) => Request::GetToplevel { id: r.new_id()? },
            (I::Surface, 6) => Request::GetPopup {
                id: r.new_id()?,
                parent: r.object()?,
            },
            (I::Surface, 7) => Request::GetA11y { id: r.new_id()? },
            (I::Surface, 8) => Request::GetPanel { id: r.new_id()? },
            (I::Toplevel, 0) => Request::SetTitle(r.string()?),
            (I::Toplevel, 1) => Request::SetAppId(r.string()?),
            (I::Toplevel, 2) => Request::SetMinSize {
                width: r.size()?,
                height: r.size()?,
            },
            (I::Toplevel, 3) => Request::SetMaxSize {
                width: r.size()?,
                height: r.size()?,
            },
            (I::Toplevel, 4) => Request::AckConfigure { serial: r.uint()? },
            (I::Toplevel, 5) => Request::ToplevelDestroy,
            (I::Popup, 0) => Request::Position {
                anchor: r.uint()?,
                gravity: r.uint()?,
                x: r.int()?,
                y: r.int()?,
            },
            (I::Popup, 1) => Request::Grab { serial: r.uint()? },
            (I::Seat, 0) => Request::SetCursor(r.string()?),
            (I::A11y, 0) => Request::TreeUpdate(r.array()?),
            (I::A11y, 1) => Request::A11yCommit,
            (I::A11y, 2) => Request::BindAction {
                node: r.uint()?,
                action: r.uint()?,
            },
            (I::Panel, 0) => Request::Layer(r.uint()?),
            (I::Panel, 1) => Request::ExclusiveZone(r.size()?),
            _ => return Err(Error::UnknownOpcode),
        };
        r.end()?;
        Ok(request)
    }

    /// The message for this request to `object`.
    pub fn encode(&self, object: u32) -> Result<Vec<u8>, Error> {
        let w = |op| Writer::new(object, op);
        let rect = |op, r: &Rect| {
            w(op)
                .int(r.x as i32)
                .int(r.y as i32)
                .int(r.w as i32)
                .int(r.h as i32)
        };
        let writer = match *self {
            Request::Bind {
                name,
                interface,
                version,
                id,
            } => w(0).uint(name).string(interface).uint(version).uint(id),
            Request::PoolCreate { size } => w(0).uint(size),
            Request::PoolBuffer {
                id,
                offset,
                width,
                height,
                stride,
                format,
            } => w(1)
                .uint(id)
                .int(offset as i32)
                .int(width as i32)
                .int(height as i32)
                .int(stride as i32)
                .uint(format as u32),
            Request::BufferDestroy => w(0),
            Request::Attach { buffer } => w(0).uint(buffer),
            Request::DamageBuffer(r) => rect(1, &r),
            Request::Frame => w(2),
            Request::SetOpaque(r) => rect(3, &r),
            Request::Commit => w(4),
            Request::GetToplevel { id } => w(5).uint(id),
            Request::GetPopup { id, parent } => w(6).uint(id).uint(parent),
            Request::GetA11y { id } => w(7).uint(id),
            Request::GetPanel { id } => w(8).uint(id),
            Request::SetTitle(text) => w(0).string(text),
            Request::SetAppId(text) => w(1).string(text),
            Request::SetMinSize { width, height } => w(2).int(width as i32).int(height as i32),
            Request::SetMaxSize { width, height } => w(3).int(width as i32).int(height as i32),
            Request::AckConfigure { serial } => w(4).uint(serial),
            Request::ToplevelDestroy => w(5),
            Request::Position {
                anchor,
                gravity,
                x,
                y,
            } => w(0).uint(anchor).uint(gravity).int(x).int(y),
            Request::Grab { serial } => w(1).uint(serial),
            Request::SetCursor(name) => w(0).string(name),
            Request::TreeUpdate(bytes) => w(0).array(bytes),
            Request::A11yCommit => w(1),
            Request::BindAction { node, action } => w(2).uint(node).uint(action),
            Request::Layer(layer) => w(0).uint(layer),
            Request::ExclusiveZone(px) => w(1).int(px as i32),
        };
        writer.finish()
    }

    /// The object this request makes, and its interface; a bind's comes from
    /// the global it names.
    fn creates(&self) -> Option<(u32, Interface)> {
        match *self {
            Request::PoolBuffer { id, .. } => Some((id, Interface::Buffer)),
            Request::GetToplevel { id } => Some((id, Interface::Toplevel)),
            Request::GetPopup { id, .. } => Some((id, Interface::Popup)),
            Request::GetA11y { id } => Some((id, Interface::A11y)),
            Request::GetPanel { id } => Some((id, Interface::Panel)),
            _ => None,
        }
    }
}

/// A buffer's rows are whole pixels apart and at least a row wide, and it starts
/// on a pixel: `stride` and `offset` are multiples of 4 and `stride` is at least
/// `width` times 4, so a renderer can take `stride / 4` as its row pitch.
fn buffer_fits(request: &Request) -> bool {
    let Request::PoolBuffer {
        offset,
        width,
        stride,
        ..
    } = *request
    else {
        return true;
    };
    stride.is_multiple_of(4)
        && offset.is_multiple_of(4)
        && width.checked_mul(4).is_some_and(|row| stride >= row)
}

/// An event to a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    // registry
    Global {
        name: u32,
        interface: &'a str,
        version: u32,
    },
    // buffer
    Release,
    // surface
    PreferredScale(u32),
    Enter,
    Leave,
    // toplevel: `states` is an array of u32 (maximized 1, fullscreen 2,
    // resizing 3, activated 4, as `xdg_toplevel.state`)
    Configure {
        serial: u32,
        width: u32,
        height: u32,
        states: &'a [u8],
    },
    Close,
    // popup
    PopupConfigure {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    PopupDone,
    // seat
    FocusIn {
        surface: u32,
    },
    FocusOut {
        surface: u32,
    },
    Modifiers(u32),
    // a11y: the agent or an assistive client asks a node to act
    ActionRequest {
        node: u32,
        action: u32,
        value: &'a str,
    },
    // panel
    PanelConfigure {
        width: u32,
        height: u32,
    },
}

impl<'a> Event<'a> {
    pub fn decode(interface: Interface, opcode: u16, args: &'a [u8]) -> Result<Self, Error> {
        use Interface as I;
        let mut r = Reader::new(args);
        let event = match (interface, opcode) {
            (I::Registry, 0) => Event::Global {
                name: r.uint()?,
                interface: r.string()?,
                version: r.uint()?,
            },
            (I::Buffer, 0) => Event::Release,
            (I::Surface, 0) => Event::PreferredScale(r.uint()?),
            (I::Surface, 1) => Event::Enter,
            (I::Surface, 2) => Event::Leave,
            (I::Toplevel, 0) => {
                let (serial, width, height) = (r.uint()?, r.size()?, r.size()?);
                let states = r.array()?;
                if !states.len().is_multiple_of(4) {
                    return Err(Error::BadArg);
                }
                Event::Configure {
                    serial,
                    width,
                    height,
                    states,
                }
            }
            (I::Toplevel, 1) => Event::Close,
            (I::Popup, 0) => Event::PopupConfigure {
                x: r.int()?,
                y: r.int()?,
                width: r.size()?,
                height: r.size()?,
            },
            (I::Popup, 1) => Event::PopupDone,
            (I::Seat, 0) => Event::FocusIn {
                surface: r.object()?,
            },
            (I::Seat, 1) => Event::FocusOut {
                surface: r.object()?,
            },
            (I::Seat, 2) => Event::Modifiers(r.uint()?),
            (I::A11y, 0) => Event::ActionRequest {
                node: r.uint()?,
                action: r.uint()?,
                value: r.string()?,
            },
            (I::Panel, 0) => Event::PanelConfigure {
                width: r.size()?,
                height: r.size()?,
            },
            _ => return Err(Error::UnknownOpcode),
        };
        r.end()?;
        Ok(event)
    }

    pub fn encode(&self, object: u32) -> Result<Vec<u8>, Error> {
        let w = |op| Writer::new(object, op);
        let writer = match *self {
            Event::Global {
                name,
                interface,
                version,
            } => w(0).uint(name).string(interface).uint(version),
            Event::Release => w(0),
            Event::PreferredScale(scale) => w(0).uint(scale),
            Event::Enter => w(1),
            Event::Leave => w(2),
            Event::Configure {
                serial,
                width,
                height,
                states,
            } => w(0)
                .uint(serial)
                .int(width as i32)
                .int(height as i32)
                .array(states),
            Event::Close => w(1),
            Event::PopupConfigure {
                x,
                y,
                width,
                height,
            } => w(0).int(x).int(y).int(width as i32).int(height as i32),
            Event::PopupDone => w(1),
            Event::FocusIn { surface } => w(0).uint(surface),
            Event::FocusOut { surface } => w(1).uint(surface),
            Event::Modifiers(mask) => w(2).uint(mask),
            Event::ActionRequest {
                node,
                action,
                value,
            } => w(0).uint(node).uint(action).string(value),
            Event::PanelConfigure { width, height } => w(0).int(width as i32).int(height as i32),
        };
        writer.finish()
    }
}

/// One connection's live objects: id to interface.
#[derive(Clone, Debug)]
pub struct Objects {
    live: Vec<(u32, Interface)>,
}

impl Default for Objects {
    fn default() -> Self {
        Self::new()
    }
}

impl Objects {
    /// A connection holding only the registry.
    pub fn new() -> Self {
        Self {
            live: alloc::vec![(REGISTRY, Interface::Registry)],
        }
    }

    pub fn interface(&self, id: u32) -> Option<Interface> {
        self.live.iter().find(|(i, _)| *i == id).map(|(_, f)| *f)
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Decodes the request in `message` (header and arguments, as
    /// [`crate::wire::split`] gives them) and applies it to the table: the
    /// target object, and the request.
    pub fn receive<'a>(
        &mut self,
        object: u32,
        opcode: u16,
        args: &'a [u8],
    ) -> Result<(Interface, Request<'a>), Error> {
        let interface = self.interface(object).ok_or(Error::UnknownObject)?;
        let request = Request::decode(interface, opcode, args)?;
        match request {
            Request::Bind {
                name,
                interface: wanted,
                version,
                id,
            } => {
                let global = usize::try_from(name)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| GLOBALS.get(n))
                    .filter(|g| g.name() == wanted)
                    .ok_or(Error::BadArg)?;
                if version == 0 || version > global.version() {
                    return Err(Error::BadArg);
                }
                self.make(id, *global)?;
            }
            Request::GetPopup { parent, .. }
                if self.interface(parent) != Some(Interface::Surface) =>
            {
                return Err(Error::BadArg);
            }
            Request::Attach { buffer } if self.interface(buffer) != Some(Interface::Buffer) => {
                return Err(Error::BadArg);
            }
            Request::BufferDestroy | Request::ToplevelDestroy => {
                self.live.retain(|(i, _)| *i != object)
            }
            _ => {
                if let Some((id, made)) = request.creates() {
                    self.make(id, made)?;
                }
            }
        }
        Ok((interface, request))
    }

    fn make(&mut self, id: u32, interface: Interface) -> Result<(), Error> {
        if id > MAX_CLIENT_ID || self.interface(id).is_some() || self.live.len() >= MAX_OBJECTS {
            return Err(Error::BadArg);
        }
        self.live.push((id, interface));
        Ok(())
    }
}
