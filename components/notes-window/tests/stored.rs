//! Notes kept in storage (P4 task 6), on compsdk's host fake of `kai:app`
//! storage: one key per note, a small checkpoint, and nothing lost on the way
//! in from `comp-notes`'s schema 2 or from the window between calls.

use comp_notes::{Actions, Notes};
use comp_notes_ui::{Entry, NEW};
use comp_notes_window::stored::{self, INDEX, V3};
use comp_notes_window::{NotesWindow, Sink};
use compsdk::Component;
use compsdk::app::fake;
use uikit::widget::{NodeId, UiError, UiEvent};

/// A sink that takes everything: these tests are about storage.
struct Quiet;

impl Sink for Quiet {
    fn open(&mut self, _: &str, _: u16, _: u16) -> Result<u32, UiError> {
        Ok(1)
    }
    fn put(&mut self, _: u32, _: &Entry) -> Result<(), UiError> {
        Ok(())
    }
    fn remove(&mut self, _: u32, _: NodeId) -> Result<(), UiError> {
        Ok(())
    }
    fn commit(&mut self, _: u32) -> Result<(), UiError> {
        Ok(())
    }
    fn close(&mut self, _: u32) -> Result<(), UiError> {
        Ok(())
    }
}

fn v3(app: &NotesWindow) -> V3 {
    let bytes = app.export(stored::SCHEMA).unwrap();
    compsdk::state::decode(&bytes, stored::SCHEMA).unwrap()
}

#[test]
fn notes_live_in_storage_and_the_checkpoint_stays_small() {
    fake::reset();
    let mut app = NotesWindow::init();
    let a = app.add("buy milk").unwrap();
    let b = app.add("call bob today").unwrap();
    fake::with(|os| {
        assert_eq!(os.storage.get(&stored::key(a)).unwrap(), b"buy milk");
        assert!(os.storage.contains_key(&stored::key(b)) && os.storage.contains_key(INDEX));
    });
    assert_eq!(v3(&app), V3 { pending: None });
    // Another instance, as after a restart, finds them in storage.
    let again = NotesWindow::init();
    assert_eq!(again.count().notes, 2);
    assert_eq!(again.get(b).as_deref(), Some("call bob today"));
    assert_eq!(again.notes.state.next_id, b + 1);
}

#[test]
fn a_schema_2_state_arrives_pending_and_the_next_add_stores_it() {
    fake::reset();
    let mut old = Notes::init();
    old.add("from the edit loop").unwrap();
    let state = old.export(2).unwrap();
    let mut app = NotesWindow::import(&state, 2).unwrap();
    // Until a call writes storage, the checkpoint keeps the notes.
    assert!(v3(&app).pending.is_some_and(|p| p.notes.len() == 1));
    let restarted = NotesWindow::import(&app.export(stored::SCHEMA).unwrap(), 3).unwrap();
    assert_eq!(restarted.count().notes, 1);
    app.add("new").unwrap();
    assert_eq!(v3(&app), V3 { pending: None });
    assert_eq!(NotesWindow::init().count().notes, 2);
    // Schema 2 is still written whole, for a rollback to comp-notes.
    let back = Notes::import(&app.export(2).unwrap(), 2).unwrap();
    assert_eq!(back.count().notes, 2);
}

#[test]
fn an_edit_from_the_window_waits_in_the_checkpoint() {
    fake::reset();
    let mut app = NotesWindow::init();
    app.present(&mut Quiet).unwrap();
    let w = app.window().unwrap();
    app.event(&mut Quiet, w, &UiEvent::Pressed(NEW)).unwrap();
    assert_eq!(app.count().notes, 1);
    fake::with(|os| assert!(os.storage.is_empty(), "no storage outside a call"));
    assert!(v3(&app).pending.is_some());
    app.add("and one more").unwrap();
    assert_eq!(v3(&app), V3 { pending: None });
    assert_eq!(NotesWindow::init().count().notes, 2);
}
