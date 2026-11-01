//! The editor's window on the host, through a real `uikit::Window`: the tree is
//! one `uikit` takes, the code view carries the rows, the gutter, the links and
//! the caret, and what the screen holds beyond the limits is cut.

use comp_editor_ui::{CODE, Change, HINT, SIZE, STATUS, TITLE, build, diff, status};
use editcore::screen::{Row, View};
use uikit::layout::FixedAdvance;
use uikit::style::Tone;
use uikit::widget::{MAX_CODE_CELLS, MAX_CODE_ROWS, Widget};
use uikit::window::Window;

fn row(n: usize, text: &str) -> Row {
    Row {
        number: Some(n),
        text: text.into(),
        selected: false,
        links: Vec::new(),
    }
}

fn view() -> View {
    let mut rows = vec![
        row(1, "fn main() {"),
        row(2, "    run(); // see src/ui.rs:7"),
        row(3, "}"),
    ];
    rows[1].selected = true;
    rows[1].links = vec![(18, 29)];
    View {
        title: "notes 2: components/notes/src/lib.rs *".into(),
        rows,
        gutter: 2,
        status: "ok built e1".into(),
        cursor: Some((1, 4)),
        mode: "edit",
        ..View::default()
    }
}

fn put(tree: &[comp_ui_bind::tree::Entry]) -> Window {
    let mut w = Window::new("Editor", SIZE.0, SIZE.1);
    for e in tree {
        w.put(e.id, e.parent, e.layout, e.widget.clone())
            .unwrap_or_else(|err| panic!("{err:?} putting {e:?}"));
    }
    w
}

#[test]
fn a_view_is_a_title_the_code_and_a_status_in_a_window_uikit_takes() {
    let tree = build(&view());
    let ids: Vec<u32> = tree.iter().map(|e| e.id).collect();
    assert_eq!(ids, [TITLE, CODE, STATUS]);
    let mut w = put(&tree);
    let frame = w.commit(&FixedAdvance);
    let name = |id| frame.a11y.iter().find(|n| n.id == id).unwrap();
    assert_eq!(name(TITLE).name, "notes 2: components/notes/src/lib.rs *");
    assert_eq!(name(STATUS).name, "ok built e1");
    // The code view is one node: its name, its rows as the value.
    assert_eq!(name(CODE).name, "notes 2: components/notes/src/lib.rs *");
    assert_eq!(
        name(CODE).value.as_deref(),
        Some("fn main() {\n    run(); // see src/ui.rs:7\n}")
    );
    // And it is told how much of it shows.
    let (_, rows, cells) = frame.code_sizes[0];
    assert!(rows > 10 && cells > 40, "{rows} x {cells}");
}

#[test]
fn the_code_view_has_the_gutter_the_caret_the_highlight_and_the_links() {
    let tree = build(&view());
    let Widget::Code(v) = &tree[1].widget else {
        panic!("a code view");
    };
    assert_eq!(v.gutter_width, 2);
    assert_eq!(v.cursor, Some((1, 4)));
    assert_eq!(v.selected, Some(1));
    assert_eq!(v.rows[0].gutter, "1");
    let spans: Vec<(u16, u16, bool, Tone)> = v.rows[1]
        .spans
        .iter()
        .map(|s| (s.from, s.to, s.link, s.tone))
        .collect();
    assert_eq!(spans, [(18, 29, true, Tone::Accent)]);
    assert!(v.rows[0].spans.is_empty());
}

#[test]
fn the_status_line_is_the_prompt_a_build_an_error_or_the_keys() {
    let mut v = view();
    assert_eq!(status(&v), ("ok built e1".into(), Tone::Muted));
    v.status_error = true;
    assert_eq!(status(&v).1, Tone::Error);
    v.building = true;
    assert_eq!(status(&v), ("building... ok built e1".into(), Tone::Accent));
    v.prompt = Some(("find", "lin".into()));
    assert_eq!(status(&v), ("find: lin_".into(), Tone::Accent));
    let empty = View::default();
    assert_eq!(status(&empty), (HINT.into(), Tone::Dim));
}

#[test]
fn what_the_screen_holds_beyond_the_limits_is_cut_so_a_put_is_never_refused() {
    let mut v = view();
    v.rows = (1..=MAX_CODE_ROWS + 50).map(|n| row(n, "x")).collect();
    v.rows[0].text = "y".repeat(MAX_CODE_CELLS + 100);
    v.rows[0].links = vec![
        (0, 5),
        (3, 3),
        (MAX_CODE_CELLS + 10, MAX_CODE_CELLS + 20),
        (MAX_CODE_CELLS - 2, MAX_CODE_CELLS + 50),
    ];
    let tree = build(&v);
    let Widget::Code(code) = &tree[1].widget else {
        panic!("a code view");
    };
    assert_eq!(code.rows.len(), MAX_CODE_ROWS);
    assert_eq!(code.rows[0].text.chars().count(), MAX_CODE_CELLS);
    // The empty span and the one past the text are gone; the last is cut to the text.
    let spans: Vec<(u16, u16)> = code.rows[0].spans.iter().map(|s| (s.from, s.to)).collect();
    assert_eq!(
        spans,
        [(0, 5), (MAX_CODE_CELLS as u16 - 2, MAX_CODE_CELLS as u16)]
    );
    let mut w = put(&tree);
    assert!(!w.commit(&FixedAdvance).list.groups.is_empty());
}

#[test]
fn a_new_line_of_text_sends_only_the_code_view_and_the_status() {
    let before = build(&view());
    let mut v = view();
    v.rows[2].text = "} // end".into();
    v.status = "ok".into();
    let changes = diff(&before, &build(&v));
    let ids: Vec<u32> = changes
        .iter()
        .map(|c| match c {
            Change::Put(e) => e.id,
            Change::Remove(id) => *id,
        })
        .collect();
    assert_eq!(ids, [CODE, STATUS]);
    assert!(diff(&before, &before).is_empty());
}
