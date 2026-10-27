//! `uikit`, the toolkit behind `kai:ui@0.1.0` (ADR 0005 section 4), without a
//! screen: an app's retained view tree, its layout, focus and input routing, the
//! display list it draws and the accessibility tree the agent reads. comphost
//! serves `kai:ui/view` from it, draws the list through `gfx2d` into the
//! window's `Surface`, and sends the tree to `desktop`; until `surfabi` lands a
//! [`surface::MemSurface`] stands in.

#![no_std]

extern crate alloc;

pub mod a11y;
mod act;
pub mod geom;
pub mod input;
pub mod layout;
pub mod list;
pub mod paint;
mod route;
mod state;
pub mod style;
pub mod surface;
pub mod tree;
pub mod widget;
pub mod window;

pub use act::ActError;
pub use window::{Frame, Window};

use alloc::collections::BTreeMap;
use widget::UiError;

/// Windows one app may have open.
pub const MAX_WINDOWS: usize = 8;

/// One app's windows, by the ids `kai:ui/view.open` returned.
#[derive(Default)]
pub struct Ui {
    windows: BTreeMap<u32, Window>,
    next: u32,
}

impl Ui {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, title: &str, width: u16, height: u16) -> Result<u32, UiError> {
        if self.windows.len() >= MAX_WINDOWS {
            return Err(UiError::TooManyWindows);
        }
        self.next = self.next.wrapping_add(1).max(1);
        while self.windows.contains_key(&self.next) {
            self.next = self.next.wrapping_add(1).max(1);
        }
        self.windows
            .insert(self.next, Window::new(title, width, height));
        Ok(self.next)
    }

    pub fn window(&mut self, id: u32) -> Result<&mut Window, UiError> {
        self.windows.get_mut(&id).ok_or(UiError::NoWindow)
    }

    pub fn close(&mut self, id: u32) -> Result<(), UiError> {
        self.windows
            .remove(&id)
            .map(|_| ())
            .ok_or(UiError::NoWindow)
    }

    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.windows.keys().copied()
    }
}
