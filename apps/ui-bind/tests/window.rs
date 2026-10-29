//! The shared window loop on the host: `Shown` opens a window once, sends only
//! what changed, keeps its last tree when the host refuses, and forgets a closed
//! window; and the diff, against a real `uikit::Window` driven through a `Sink`.

use comp_ui_bind::tree::{Change, Entry, column, diff, entry, label, row, wide};
use comp_ui_bind::{Shown, Sink};
use uikit::layout::FixedAdvance as M;
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Insets, Layout, NodeId, ROOT, TextField, UiError, Widget};
use uikit::{Frame, Window};

/// `kai:ui/view` over a real window, counting what the app asked.
#[derive(Default)]
struct Host {
    window: Option<Window>,
    opened: u32,
    puts: u32,
    removes: u32,
    commits: u32,
    closed: u32,
    /// The next put is refused.
    refuse_put: bool,
}

impl Sink for Host {
    fn open(&mut self, title: &str, width: u16, height: u16) -> Result<u32, UiError> {
        self.opened += 1;
        self.window = Some(Window::new(title, width, height));
        Ok(7)
    }

    fn put(&mut self, window: u32, e: &Entry) -> Result<(), UiError> {
        assert_eq!(window, 7);
        if std::mem::take(&mut self.refuse_put) {
            return Err(UiError::TooBig);
        }
        self.puts += 1;
        let w = self.window.as_mut().ok_or(UiError::NoWindow)?;
        w.put(e.id, e.parent, e.layout, e.widget.clone())
    }

    fn remove(&mut self, _: u32, node: NodeId) -> Result<(), UiError> {
        self.removes += 1;
        self.window.as_mut().ok_or(UiError::NoWindow)?.remove(node)
    }

    fn commit(&mut self, _: u32) -> Result<(), UiError> {
        self.commits += 1;
        Ok(())
    }

    fn close(&mut self, _: u32) -> Result<(), UiError> {
        self.closed += 1;
        self.window = None;
        Ok(())
    }
}

fn frame(host: &mut Host) -> Frame {
    host.window.as_mut().unwrap().commit(&M)
}

fn tree(n: u32, title: &str) -> Vec<Entry> {
    let mut tree = vec![
        entry(1, ROOT, column(8, 4), Widget::Stack { axis: Axis::Column }),
        entry(2, 1, wide(), label(title, TypeStyle::Heading, Tone::Normal)),
        entry(3, 1, wide(), row()),
    ];
    for i in 0..n {
        tree.push(entry(
            10 + i,
            3,
            Layout::FIT,
            label(&format!("item {i}"), TypeStyle::Body, Tone::Muted),
        ));
    }
    tree
}

#[test]
fn present_opens_once_sends_the_diff_and_commits() {
    let mut host = Host::default();
    let mut shown = Shown::new();
    assert_eq!(shown.id(), None);
    shown
        .present(&mut host, "Demo", (200, 100), tree(2, "a"))
        .unwrap();
    assert_eq!((host.opened, host.puts, host.commits), (1, 5, 1));
    assert_eq!(shown.id(), Some(7));

    // The same tree again sends nothing, but still commits.
    shown
        .present(&mut host, "Demo", (200, 100), tree(2, "a"))
        .unwrap();
    assert_eq!(
        (host.opened, host.puts, host.removes, host.commits),
        (1, 5, 0, 2)
    );

    // A change sends that node alone; a shorter list removes the extra.
    shown
        .present(&mut host, "Demo", (200, 100), tree(2, "b"))
        .unwrap();
    assert_eq!(host.puts, 6);
    shown
        .present(&mut host, "Demo", (200, 100), tree(1, "b"))
        .unwrap();
    assert_eq!(host.removes, 1);

    // What the host holds is what a window built from the tree draws.
    let mut fresh = Host::default();
    Shown::new()
        .present(&mut fresh, "Demo", (200, 100), tree(1, "b"))
        .unwrap();
    assert_eq!(frame(&mut host).list, frame(&mut fresh).list);
}

#[test]
fn a_refused_put_keeps_the_last_tree_so_the_next_present_sends_it_again() {
    let mut host = Host::default();
    let mut shown = Shown::new();
    shown
        .present(&mut host, "Demo", (200, 100), tree(1, "a"))
        .unwrap();
    host.refuse_put = true;
    assert_eq!(
        shown.present(&mut host, "Demo", (200, 100), tree(1, "b")),
        Err(UiError::TooBig)
    );
    shown
        .present(&mut host, "Demo", (200, 100), tree(1, "b"))
        .unwrap();
    let mut fresh = Host::default();
    Shown::new()
        .present(&mut fresh, "Demo", (200, 100), tree(1, "b"))
        .unwrap();
    assert_eq!(frame(&mut host).list, frame(&mut fresh).list);
}

#[test]
fn a_window_that_cannot_open_is_an_error_and_a_closed_one_is_forgotten() {
    struct Full;
    impl Sink for Full {
        fn open(&mut self, _: &str, _: u16, _: u16) -> Result<u32, UiError> {
            Err(UiError::TooManyWindows)
        }
        fn put(&mut self, _: u32, _: &Entry) -> Result<(), UiError> {
            unreachable!()
        }
        fn remove(&mut self, _: u32, _: NodeId) -> Result<(), UiError> {
            unreachable!()
        }
        fn commit(&mut self, _: u32) -> Result<(), UiError> {
            unreachable!()
        }
        fn close(&mut self, _: u32) -> Result<(), UiError> {
            unreachable!()
        }
    }
    let mut shown = Shown::new();
    assert_eq!(
        shown.present(&mut Full, "Demo", (1, 1), tree(1, "a")),
        Err(UiError::TooManyWindows)
    );
    assert_eq!(shown.id(), None);

    let mut host = Host::default();
    shown
        .present(&mut host, "Demo", (200, 100), tree(1, "a"))
        .unwrap();
    // Someone else's window is not ours to close.
    assert_eq!(shown.closed(&mut host, 99), Ok(false));
    assert_eq!(host.closed, 0);
    assert_eq!(shown.closed(&mut host, 7), Ok(true));
    assert_eq!((host.closed, shown.id()), (1, None));
    // Presenting again opens a new window with the whole tree.
    shown
        .present(&mut host, "Demo", (200, 100), tree(1, "a"))
        .unwrap();
    assert_eq!(host.opened, 2);
}

/// Applies `changes` to a window.
fn apply(w: &mut Window, changes: &[Change]) {
    for c in changes {
        match c {
            Change::Put(e) => w.put(e.id, e.parent, e.layout, e.widget.clone()).unwrap(),
            Change::Remove(id) => w.remove(*id).unwrap(),
        }
    }
}

#[test]
fn the_diff_rebuilds_the_new_tree_from_any_old_one() {
    let field = |value: &str| {
        entry(
            4,
            1,
            wide(),
            Widget::TextLine(TextField {
                label: String::from("Name"),
                value: String::from(value),
                placeholder: String::new(),
                max_len: 20,
            }),
        )
    };
    let mut trees: Vec<Vec<Entry>> = vec![Vec::new(), tree(0, "x"), tree(3, "x"), tree(1, "y")];
    let mut with_field = tree(2, "z");
    with_field.insert(3, field("hi"));
    trees.push(with_field.clone());
    with_field[3] = field("hello");
    trees.push(with_field);
    let padded = Layout {
        padding: Insets::all(3),
        ..Layout::FILL
    };
    let mut moved = tree(2, "z");
    moved[2] = entry(3, 1, padded, row());
    trees.push(moved);
    for old in &trees {
        for new in &trees {
            let mut w = Window::new("T", 200, 100);
            apply(&mut w, &diff(&[], old));
            w.commit(&M);
            apply(&mut w, &diff(old, new));
            let mut fresh = Window::new("T", 200, 100);
            apply(&mut fresh, &diff(&[], new));
            assert_eq!(w.commit(&M).list, fresh.commit(&M).list);
        }
    }
    assert!(diff(&trees[2], &trees[2]).is_empty());
}
