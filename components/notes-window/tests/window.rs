//! The window's glue on the host, through a `Sink` that is a real `uikit::Ui`.

use comp_notes::Actions;
use comp_notes_ui::{Entry, NEW};
use comp_notes_window::{NotesWindow, Sink, TITLE};
use compsdk::Component;
use uikit::Ui;
use uikit::layout::FixedAdvance;
use uikit::widget::{NodeId, UiError, UiEvent};

#[derive(Default)]
struct Host {
    ui: Ui,
    calls: Vec<&'static str>,
    /// Puts refused from this many calls on.
    fail_after: Option<usize>,
}

impl Sink for Host {
    fn open(&mut self, title: &str, width: u16, height: u16) -> Result<u32, UiError> {
        self.calls.push("open");
        self.ui.open(title, width, height)
    }

    fn put(&mut self, window: u32, e: &Entry) -> Result<(), UiError> {
        self.calls.push("put");
        if self.fail_after.is_some_and(|n| self.calls.len() > n) {
            return Err(UiError::TooBig);
        }
        let w = self.ui.window(window)?;
        w.put(e.id, e.parent, e.layout, e.widget.clone())
    }

    fn remove(&mut self, window: u32, node: NodeId) -> Result<(), UiError> {
        self.calls.push("remove");
        self.ui.window(window)?.remove(node)
    }

    fn commit(&mut self, window: u32) -> Result<(), UiError> {
        self.calls.push("commit");
        self.ui.window(window)?.commit(&FixedAdvance);
        Ok(())
    }

    fn close(&mut self, window: u32) -> Result<(), UiError> {
        self.calls.push("close");
        self.ui.close(window)
    }
}

fn names(host: &mut Host, window: u32) -> Vec<String> {
    let frame = host.ui.window(window).unwrap().commit(&FixedAdvance);
    assert_eq!(frame.title, TITLE);
    frame.a11y.into_iter().map(|n| n.name).collect()
}

#[test]
fn it_opens_once_and_then_sends_only_what_changed() {
    let mut host = Host::default();
    let mut app = NotesWindow::init();
    app.present(&mut host).unwrap();
    let w = app.window().unwrap();
    let first = host.calls.len();
    app.present(&mut host).unwrap();
    // Nothing changed: no puts, just the commit.
    assert_eq!(&host.calls[first..], ["commit"]);
    assert_eq!(host.calls.iter().filter(|c| **c == "open").count(), 1);
    app.notes.add("milk and eggs").unwrap();
    app.present(&mut host).unwrap();
    assert!(names(&mut host, w).iter().any(|n| n.contains("milk")));
}

#[test]
fn a_click_adds_a_note_and_a_close_request_closes() {
    let mut host = Host::default();
    let mut app = NotesWindow::init();
    app.present(&mut host).unwrap();
    let w = app.window().unwrap();
    app.event(&mut host, w, &UiEvent::Pressed(NEW)).unwrap();
    assert_eq!(app.count().notes, 1);
    // Another window's events are not this one's.
    app.event(&mut host, w + 1, &UiEvent::Pressed(NEW)).unwrap();
    assert_eq!(app.count().notes, 1);
    app.event(&mut host, w, &UiEvent::CloseRequested).unwrap();
    assert_eq!(
        (app.window(), host.ui.window(w).err()),
        (None, Some(UiError::NoWindow))
    );
    // Shown again, it reopens with the whole tree.
    app.present(&mut host).unwrap();
    assert_ne!(app.window(), None);
}

#[test]
fn a_refused_put_keeps_the_last_tree_so_the_next_try_resends() {
    let mut host = Host::default();
    let mut app = NotesWindow::init();
    app.present(&mut host).unwrap();
    let w = app.window().unwrap();
    app.notes.add("one").unwrap();
    host.fail_after = Some(host.calls.len());
    assert_eq!(app.present(&mut host), Err(UiError::TooBig));
    host.fail_after = None;
    app.present(&mut host).unwrap();
    assert!(names(&mut host, w).iter().any(|n| n.contains("one")));
}
