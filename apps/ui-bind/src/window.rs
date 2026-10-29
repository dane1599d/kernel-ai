//! A window's open-diff-commit loop, as Notes' window runs it: open the window
//! when there is none, send what changed since the last tree, commit. The calls
//! go through a [`Sink`]: `kai:ui/view` in WebAssembly ([`crate::bind::View`]),
//! and a test's own on the host.

use crate::tree::{Change, Entry, diff};
use alloc::vec::Vec;
use uikit::widget::{NodeId, UiError};

/// `kai:ui/view`, as an app uses it.
pub trait Sink {
    fn open(&mut self, title: &str, width: u16, height: u16) -> Result<u32, UiError>;
    fn put(&mut self, window: u32, entry: &Entry) -> Result<(), UiError>;
    fn remove(&mut self, window: u32, node: NodeId) -> Result<(), UiError>;
    fn commit(&mut self, window: u32) -> Result<(), UiError>;
    fn close(&mut self, window: u32) -> Result<(), UiError>;
}

/// The window an app has open, and the tree it was last sent.
#[derive(Debug, Default)]
pub struct Shown {
    window: Option<(u32, Vec<Entry>)>,
}

impl Shown {
    pub const fn new() -> Self {
        Self { window: None }
    }

    /// The open window's id.
    pub fn id(&self) -> Option<u32> {
        self.window.as_ref().map(|(id, _)| *id)
    }

    /// Opens the window if it is closed, sends what changed to reach `tree` and
    /// commits. On an error the last tree is kept, so the next call sends the
    /// changes again.
    pub fn present(
        &mut self,
        sink: &mut dyn Sink,
        title: &str,
        size: (u16, u16),
        tree: Vec<Entry>,
    ) -> Result<(), UiError> {
        let (id, shown) = match self.window.take() {
            Some(open) => open,
            None => (sink.open(title, size.0, size.1)?, Vec::new()),
        };
        let sent = send(sink, id, &diff(&shown, &tree)).and_then(|()| sink.commit(id));
        self.window = Some((id, if sent.is_ok() { tree } else { shown }));
        sent
    }

    /// The window `window` was closed by the app or the user: forgets it, and
    /// closes it on the host if `close` says the host still has it. False if
    /// `window` is not this app's open one.
    pub fn closed(&mut self, sink: &mut dyn Sink, window: u32) -> Result<bool, UiError> {
        if self.id() != Some(window) {
            return Ok(false);
        }
        self.window = None;
        sink.close(window).map(|()| true)
    }
}

/// Sends `changes` to `window`, stopping at the first the host refuses.
pub fn send(sink: &mut dyn Sink, window: u32, changes: &[Change]) -> Result<(), UiError> {
    changes.iter().try_for_each(|c| match c {
        Change::Put(e) => sink.put(window, e),
        Change::Remove(node) => sink.remove(window, *node),
    })
}
