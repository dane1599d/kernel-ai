//! Notes with its window: the app P4 ships, where `comp-notes` alone is the
//! edit loop's (M9) build. [`NotesWindow`] is `comp-notes`'s [`Notes`] with the
//! window's view state; it opens its window when it starts, and after each
//! event or change of state it sends `comp-notes-ui`'s [`diff`] of the tree
//! and commits. The calls go through a [`Sink`]: `kai:ui/view` in WebAssembly
//! (`bind`), and a test's own on the host.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;
pub mod stored;

use alloc::string::String;
use alloc::vec::Vec;
use comp_notes::{Actions, Notes, Tally};
use comp_notes_ui::{self as ui, Change, Entry, View, build, diff};
use compsdk::{Component, ImportError};
use uikit::widget::{NodeId, UiError, UiEvent};

/// The window's title.
pub const TITLE: &str = "Notes";

/// `kai:ui/view`, as the window uses it.
pub trait Sink {
    fn open(&mut self, title: &str, width: u16, height: u16) -> Result<u32, UiError>;
    fn put(&mut self, window: u32, entry: &Entry) -> Result<(), UiError>;
    fn remove(&mut self, window: u32, node: NodeId) -> Result<(), UiError>;
    fn commit(&mut self, window: u32) -> Result<(), UiError>;
    fn close(&mut self, window: u32) -> Result<(), UiError>;
}

pub struct NotesWindow {
    pub notes: Notes,
    pub view: View,
    /// Whether `notes` holds notes storage does not have yet ([`stored`]).
    pending: bool,
    /// The open window, and the tree it was last sent.
    window: Option<(u32, Vec<Entry>)>,
}

impl NotesWindow {
    fn new(notes: Notes) -> Self {
        Self {
            notes,
            view: View::default(),
            pending: false,
            window: None,
        }
    }

    /// The open window's id.
    pub fn window(&self) -> Option<u32> {
        self.window.as_ref().map(|(id, _)| *id)
    }

    /// Opens the window if it is closed, sends what changed and commits. On an
    /// error the last tree is kept, so the next call sends the changes again.
    pub fn present(&mut self, sink: &mut dyn Sink) -> Result<(), UiError> {
        let (id, shown) = match self.window.take() {
            Some(open) => open,
            None => {
                let (w, h) = ui::SIZE;
                (sink.open(TITLE, w, h)?, Vec::new())
            }
        };
        let tree = build(&self.notes, &self.view);
        let sent = send(sink, id, &diff(&shown, &tree)).and_then(|()| sink.commit(id));
        self.window = Some((id, if sent.is_ok() { tree } else { shown }));
        sent
    }

    /// One event from `window`: a close request closes it, and anything that
    /// changes the tree is sent.
    pub fn event(&mut self, sink: &mut dyn Sink, window: u32, e: &UiEvent) -> Result<(), UiError> {
        if self.window() != Some(window) {
            return Ok(());
        }
        if *e == UiEvent::CloseRequested {
            self.window = None;
            return sink.close(window);
        }
        if ui::on_ui(&mut self.notes, &mut self.view, e) {
            // Storage is written in action calls only: the change waits.
            self.pending = true;
            return self.present(sink);
        }
        Ok(())
    }
}

fn send(sink: &mut dyn Sink, window: u32, changes: &[Change]) -> Result<(), UiError> {
    changes.iter().try_for_each(|c| match c {
        Change::Put(e) => sink.put(window, e),
        Change::Remove(node) => sink.remove(window, *node),
    })
}

/// Shows the window, or logs why it could not.
fn shown(mut app: NotesWindow) -> NotesWindow {
    app.present_logged();
    app
}

impl NotesWindow {
    fn present_logged(&mut self) {
        #[cfg(target_family = "wasm")]
        if let Err(e) = self.present(&mut bind::View) {
            compsdk::log(&alloc::format!("notes window: {e:?}"));
        }
    }
}

impl NotesWindow {
    /// The notes storage holds, read through the host's view.
    fn from_storage() -> Self {
        Self::new(Notes {
            state: stored::load(),
        })
    }

    /// Writes pending notes out; only inside an action call.
    fn store(&mut self) {
        stored::save(&self.notes.state);
        self.pending = false;
    }
}

impl Component for NotesWindow {
    const SCHEMA: u32 = stored::SCHEMA;

    fn init() -> Self {
        shown(Self::from_storage())
    }

    /// Schema 3 reads its notes from storage, or its pending ones; schemas 1
    /// and 2 (`comp-notes`'s) arrive pending, written out by the next `add`.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != stored::SCHEMA {
            let mut app = Self::new(Notes::import(state, from_schema)?);
            app.pending = true;
            return Ok(shown(app));
        }
        let v3: stored::V3 =
            compsdk::state::decode(state, stored::SCHEMA).ok_or(ImportError::Bad)?;
        let app = match v3.pending {
            Some(state) => Self {
                pending: true,
                ..Self::new(Notes { state })
            },
            None => Self::from_storage(),
        };
        Ok(shown(app))
    }

    /// Schema 3 carries only pending notes; schema 2 is the whole state, for a
    /// rollback to `comp-notes`.
    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        if schema != stored::SCHEMA {
            return self.notes.export(schema);
        }
        let v3 = stored::V3 {
            pending: self.pending.then(|| self.notes.state.clone()),
        };
        Some(compsdk::state::encode(stored::SCHEMA, &v3))
    }

    fn health(&self) -> u32 {
        self.notes.health()
    }

    fn self_test(&self) -> u32 {
        self.notes.self_test()
    }
}

impl Actions for NotesWindow {
    fn add(&mut self, text: &str) -> Result<u32, String> {
        let id = self.notes.add(text)?;
        self.store();
        self.present_logged();
        Ok(id)
    }

    fn count(&self) -> Tally {
        self.notes.count()
    }

    fn find(&self, word: &str) -> Vec<u32> {
        self.notes.find(word)
    }

    fn get(&self, id: u32) -> Option<String> {
        self.notes.get(id)
    }

    fn about(&self) -> String {
        self.notes.about()
    }
}

comp_notes::app!(NotesWindow);
