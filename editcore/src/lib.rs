//! The edit loop (M9b), without system calls: what the ring-3 `edit` service and
//! the host's build service share, tested on the host.
//!
//! The founder types `:edit notes` in the terminal. The kernel sends each line to
//! the edit service, which keeps a [`session::Session`] over the running version's
//! source, checks the [`rules`], sends the edited source to the build service
//! ([`wire`]), checks the artifact that comes back and uploads it. The kernel then
//! shows the change from its own store and asks before it swaps.

#![no_std]

extern crate alloc;

pub mod command;
pub mod gate;
pub mod ipc;
pub mod journal;
pub mod rules;
pub mod scenario;
pub mod screen;
pub mod service;
pub mod session;
#[cfg(test)]
mod tests;
pub mod wire;

/// Most files in an editable bundle.
pub const MAX_FILES: usize = kabi::live::UPLOAD_FILES_MAX;
/// Most lines in one file.
pub const MAX_LINES: usize = kabi::live::UPLOAD_LINES_MAX;
/// Largest source bundle sent to the build service.
pub const MAX_BUNDLE: usize = kabi::live::UPLOAD_SOURCE_MAX;
/// Most compiler output the build service returns.
pub const MAX_LOG: usize = 6 * 1024;
/// Most lines one reply prints.
pub const MAX_PRINTED: usize = 40;

/// A component whose source can be edited, and where it lives in the workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Editable {
    pub component: &'static str,
    /// The crate the build service builds.
    pub krate: &'static str,
    /// Its directory; only `.rs` files under its `src` may change.
    pub dir: &'static str,
}

pub const EDITABLE: &[Editable] = &[Editable {
    component: "notes",
    krate: "comp-notes",
    dir: "components/notes",
}];

pub fn editable(component: &str) -> Option<&'static Editable> {
    EDITABLE.iter().find(|e| e.component == component)
}
