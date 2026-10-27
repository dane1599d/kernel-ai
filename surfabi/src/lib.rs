//! `kai-surface 1`: the window protocol between an app and `desktop` (ADR 0005
//! §3), without system calls, so every parser here is tested on the host and
//! with hostile input.
//!
//! - [`wire`]: Wayland's 8-byte header and 32-bit arguments, at most
//!   [`MAX_MESSAGE`] bytes a message. A bad size or a bad argument is an error,
//!   and `desktop` closes the connection on any error.
//! - [`proto`]: the objects (registry, pool, buffer, surface, toplevel, popup,
//!   seat, a11y, panel) and their requests and events, decoded into types.
//! - [`header`]: the 4 KiB page at the start of every pool's `Shm`: the format,
//!   the front buffer, `commit_seq`, up to 32 damage rectangles and a ring of 64
//!   events, so a frame costs a doorbell and no message.
//! - [`tree`]: the accessibility tree records `a11y.tree_update` carries, a
//!   pinned subset of AccessKit 0.25.1's `TreeUpdate`.
//! - [`input`]: ADR 0007's 16-byte `InputEvent` from `kabi`, which the header
//!   page's events carry.
//!
//! The objects follow Wayland's, so a Wayland client or a guest bridge maps onto
//! them one to one. Where ADR 0005's table leaves out how an object is made,
//! this crate follows Wayland: globals are bound from the registry, and a
//! surface makes its role objects (toplevel, popup, a11y, panel) as
//! `xdg_surface` does.

#![no_std]

extern crate alloc;

pub mod header;
pub mod proto;
pub mod tree;
pub mod wire;

/// ADR 0007's `InputEvent`, the kernel's (INPUT_READ) and the header page's.
pub use kabi::input;

#[cfg(test)]
mod tests;

/// The protocol's name and version, as the registry announces them.
pub const PROTOCOL: &str = "kai-surface";
pub const VERSION: u32 = 1;

/// The longest message, header included: Wayland's `WL_MAX_MESSAGE_SIZE`.
pub const MAX_MESSAGE: usize = 4096;

/// The pixel formats every surface may use, with Wayland's `wl_shm.format`
/// codes. ARGB8888 is premultiplied, as Wayland's is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Format {
    Argb8888 = 0,
    Xrgb8888 = 1,
}

impl Format {
    pub fn from_raw(raw: u32) -> Option<Format> {
        match raw {
            0 => Some(Format::Argb8888),
            1 => Some(Format::Xrgb8888),
            _ => None,
        }
    }

    pub const fn bytes_per_pixel(self) -> u32 {
        4
    }
}

/// A rectangle in buffer pixels, as damage and opaque regions give it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// This rectangle cut to `width` x `height`; `None` if nothing is left.
    pub fn clip(self, width: u32, height: u32) -> Option<Rect> {
        let x2 = self.x.saturating_add(self.w).min(width);
        let y2 = self.y.saturating_add(self.h).min(height);
        (self.x < x2 && self.y < y2).then(|| Rect {
            x: self.x,
            y: self.y,
            w: x2 - self.x,
            h: y2 - self.y,
        })
    }
}
