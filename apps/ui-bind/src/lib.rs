//! What every app with a window shares: the `kai:ui` tree's nodes and diff
//! ([`tree`]), the window's open-diff-commit loop ([`window`]) and, on wasm32,
//! the bindings to `kai:ui/view` and `kai:ui/input` ([`bind`]).
//!
//! An app is three crates: its model (no `kai:ui`, host-tested), its window (a
//! tree built from the model, host-tested through `uikit`), and the component,
//! which binds them to the OS. The component implements [`OnUi`] and calls
//! [`input!`] once; its own `actions` interface it binds with its own
//! `wit_bindgen::generate!`, as `comp-notes` does.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
pub mod bind;
pub mod tree;
pub mod window;

pub use window::{Shown, Sink};

#[doc(hidden)]
pub mod __private {
    pub use alloc::format;
    pub use compsdk;
}

use uikit::widget::{UiError, UiEvent};

/// What an app does with an event from its window. [`input!`] binds it to
/// `kai:ui/input`.
pub trait OnUi {
    /// `event` came from `window`. `sink` is `kai:ui/view`, to redraw with.
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError>;
}
