//! The code view: what it draws, how big it says it is, the keys, clicks and
//! wheel it hands the app, and the limits an app is held to.

use uikit::Window;
use uikit::code;
use uikit::geom::{Point, Rect};
use uikit::input::{Input, Key, Mods};
use uikit::layout::FixedAdvance as M;
use uikit::list::Op;
use uikit::style::Tone;
use uikit::style::metric::CODE_PAD;
use uikit::style::{Token, TypeStyle};
use uikit::widget::{
    CodeRow, CodeSpan, CodeView, Layout, MAX_CODE_CELLS, MAX_CODE_ROWS, ROOT, UiError, UiEvent,
    Widget,
};

const NODE: u32 = 5;

fn row(text: &str, gutter: &str) -> CodeRow {
    CodeRow {
        text: text.into(),
        gutter: gutter.into(),
        spans: Vec::new(),
    }
}

fn view(rows: Vec<CodeRow>) -> CodeView {
    CodeView {
        label: "notes 2: lib.rs".into(),
        gutter_width: 4,
        rows,
        cursor: Some((1, 3)),
        selected: Some(1),
    }
}

fn window(v: CodeView) -> Window {
    let mut w = Window::new("Editor", 400, 300);
    w.put(NODE, ROOT, Layout::FILL, Widget::Code(v)).unwrap();
    w.focus(NODE).unwrap();
    w
}

fn press(key: Key, mods: u8, text: &str) -> Input {
    Input::Key {
        key,
        down: true,
        mods: Mods(mods),
        text: text.into(),
    }
}

fn sample() -> CodeView {
    view(vec![
        row("fn main() {", "  1"),
        row("    run();", "  2"),
        row("}", "  3"),
    ])
}

#[test]
fn a_code_view_is_one_node_with_its_rows_as_the_value() {
    let mut w = window(sample());
    let frame = w.commit(&M);
    let node = frame.a11y.iter().find(|n| n.id == NODE).unwrap();
    assert_eq!(node.name, "notes 2: lib.rs");
    assert_eq!(node.value.as_deref(), Some("fn main() {\n    run();\n}"));
    // The rows are not nodes of their own.
    assert_eq!(frame.a11y.len(), 2);
}

#[test]
fn the_value_is_cut_to_what_the_agent_may_read() {
    let rows = (0..100).map(|i| row(&"x".repeat(150 + i), "")).collect();
    let mut w = window(view(rows));
    let frame = w.commit(&M);
    let node = frame.a11y.iter().find(|n| n.id == NODE).unwrap();
    assert!(node.value.as_ref().unwrap().len() <= code::A11Y_BYTES);
}

#[test]
fn it_draws_mono_rows_a_dim_gutter_a_highlighted_row_and_the_caret() {
    let mut w = window(sample());
    let frame = w.commit(&M);
    let group = frame.list.groups.iter().find(|g| g.node == NODE).unwrap();
    let texts: Vec<(&str, Token, TypeStyle)> = group
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Text {
                text, token, style, ..
            } => Some((text.as_str(), *token, *style)),
            _ => None,
        })
        .collect();
    assert!(texts.contains(&("  1", Token::Dim, TypeStyle::Mono)));
    assert!(texts.contains(&("    run();", Token::Text, TypeStyle::Mono)));
    // The selected row is raised; the caret is a 2 px bar at row 1, cell 3.
    assert!(group.ops.iter().any(|op| matches!(
        op,
        Op::Fill { token: Token::Raised, rect, .. } if rect.h == code::row_h(&M)
    )));
    let cw = code::cell_w(&M);
    let rh = code::row_h(&M);
    let caret = group.ops.iter().find_map(|op| match op {
        Op::Fill {
            rect,
            token: Token::Text,
            ..
        } if rect.w == 2 => Some(*rect),
        _ => None,
    });
    assert_eq!(
        caret,
        Some(Rect::new(CODE_PAD + 4 * cw + 3 * cw, CODE_PAD + rh, 2, rh))
    );
}

#[test]
fn spans_colour_runs_and_underline_links() {
    let mut v = sample();
    v.rows[1].spans = vec![
        CodeSpan {
            from: 4,
            to: 7,
            tone: Tone::Accent,
            link: false,
        },
        CodeSpan {
            from: 7,
            to: 10,
            tone: Tone::Normal,
            link: true,
        },
    ];
    let mut w = window(v);
    let frame = w.commit(&M);
    let group = frame.list.groups.iter().find(|g| g.node == NODE).unwrap();
    let runs: Vec<(&str, Token)> = group
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Text { text, token, .. } if text.contains("run") || text.contains("()") => {
                Some((text.as_str(), *token))
            }
            Op::Text { text, token, .. } if text == "    " => Some((text.as_str(), *token)),
            _ => None,
        })
        .collect();
    assert!(runs.contains(&("    ", Token::Text)), "{runs:?}");
    assert!(runs.contains(&("run", Tone::Accent.token())), "{runs:?}");
    assert!(runs.contains(&("();", Token::Accent)), "{runs:?}");
    // The link has a line under it, three cells wide.
    let cw = code::cell_w(&M);
    assert!(group.ops.iter().any(|op| matches!(
        op,
        Op::Fill { rect, token: Token::Accent, .. } if rect.h == 1 && rect.w == 3 * cw
    )));
}

#[test]
fn it_says_how_many_rows_and_cells_it_shows() {
    let mut w = window(sample());
    let frame = w.commit(&M);
    let (_, rows, cells) = frame.code_sizes[0];
    // The window is 400 by 300 logical px; the root fills it and the view fills the root.
    let r = Rect::new(0, 0, 400, 300);
    assert_eq!((rows, cells), code::grid(&M, r, 4));
    assert!(rows > 5 && cells > 20);
    // A smaller window shows fewer.
    w.resize(200, 100);
    let (_, smaller_rows, smaller_cells) = w.commit(&M).code_sizes[0];
    assert!(smaller_rows < rows && smaller_cells < cells);
}

#[test]
fn every_key_reaches_the_app_by_name_with_its_text_and_modifiers() {
    let mut w = window(sample());
    let a = |name: &str, text: &str, mods: u8| UiEvent::Key {
        node: NODE,
        name: name.into(),
        text: text.into(),
        mods,
    };
    // Typing: the letter's name and what it types.
    assert_eq!(
        w.input(press(Key::Other(0x04), 0, "a"), &M),
        [a("a", "a", 0)]
    );
    assert_eq!(
        w.input(press(Key::Other(0x04), 1, "A"), &M),
        [a("a", "A", 1)]
    );
    // A command key types nothing, whatever the text says.
    assert_eq!(
        w.input(press(Key::Other(0x16), 2, "\u{13}"), &M),
        [a("s", "", 2)]
    );
    assert_eq!(
        w.input(press(Key::Other(0x16), 2, "s"), &M),
        [a("s", "", 2)]
    );
    // Tab and Escape are the app's, not the focus ring's.
    assert_eq!(w.input(press(Key::Tab, 0, "\t"), &M), [a("tab", "", 0)]);
    assert_eq!(w.input(press(Key::Escape, 0, ""), &M), [a("esc", "", 0)]);
    assert_eq!(w.input(press(Key::Enter, 0, "\n"), &M), [a("enter", "", 0)]);
    assert_eq!(
        w.input(press(Key::PageDown, 0, ""), &M),
        [a("pagedown", "", 0)]
    );
    assert_eq!(
        w.input(press(Key::Other(0x3A), 0, ""), &M),
        [a("f1", "", 0)]
    );
    // A modifier alone says nothing; Super is `desktop`'s.
    assert!(w.input(press(Key::Other(0xE0), 2, ""), &M).is_empty());
    assert!(w.input(press(Key::Other(0x04), 8, ""), &M).is_empty());
    // Releasing a key says nothing.
    let up = Input::Key {
        key: Key::Other(0x04),
        down: false,
        mods: Mods(0),
        text: String::new(),
    };
    assert!(w.input(up, &M).is_empty());
}

#[test]
fn ctrl_tab_leaves_a_code_view_for_the_next_control_and_comes_back() {
    const BUTTON: u32 = 6;
    let mut w = window(sample());
    w.put(
        BUTTON,
        ROOT,
        Layout::FIT,
        Widget::Button {
            text: "Build".into(),
            action: None,
        },
    )
    .unwrap();
    assert_eq!(w.focus_node(), Some(NODE));
    // The key is the focus ring's: the app is told nothing.
    assert!(w.input(press(Key::Tab, Mods::CTRL, "	"), &M).is_empty());
    assert_eq!(w.focus_node(), Some(BUTTON));
    // From the button the ring wraps back to the code view.
    assert!(w.input(press(Key::Tab, Mods::CTRL, "	"), &M).is_empty());
    assert_eq!(w.focus_node(), Some(NODE));
    // With Shift it goes the other way round.
    let both = Mods::CTRL | Mods::SHIFT;
    assert!(w.input(press(Key::Tab, both, "	"), &M).is_empty());
    assert_eq!(w.focus_node(), Some(BUTTON));
    assert!(w.input(press(Key::Tab, both, "	"), &M).is_empty());
    assert_eq!(w.focus_node(), Some(NODE));
    // Plain Tab, and Tab with Alt, are still the app's.
    let tab = |mods: u8| UiEvent::Key {
        node: NODE,
        name: "tab".into(),
        text: String::new(),
        mods,
    };
    assert_eq!(w.input(press(Key::Tab, 0, "	"), &M), [tab(0)]);
    assert_eq!(
        w.input(press(Key::Tab, Mods::CTRL | Mods::ALT, "	"), &M),
        [tab(Mods::CTRL | Mods::ALT)]
    );
    assert_eq!(w.focus_node(), Some(NODE));
}

#[test]
fn a_lone_code_view_keeps_focus_on_ctrl_tab_and_still_tells_the_app_nothing() {
    let mut w = window(sample());
    assert!(w.input(press(Key::Tab, Mods::CTRL, "	"), &M).is_empty());
    assert_eq!(w.focus_node(), Some(NODE));
}

#[test]
fn a_view_that_does_not_have_focus_gives_the_app_no_keys() {
    let mut w = Window::new("Editor", 400, 300);
    w.put(NODE, ROOT, Layout::FILL, Widget::Code(sample()))
        .unwrap();
    assert!(w.input(press(Key::Other(0x04), 0, "a"), &M).is_empty());
}

#[test]
fn a_click_says_the_row_and_cell_and_a_ctrl_click_says_so() {
    let mut w = window(sample());
    w.commit(&M);
    let (cw, rh) = (code::cell_w(&M), code::row_h(&M));
    let at = Point::new(CODE_PAD + (4 + 6) * cw + 1, CODE_PAD + 2 * rh + 1);
    let click = |w: &mut Window, mods: u8| {
        let mut out = w.input(
            Input::Button {
                at,
                down: true,
                mods: Mods(mods),
            },
            &M,
        );
        out.extend(w.input(
            Input::Button {
                at,
                down: false,
                mods: Mods(mods),
            },
            &M,
        ));
        out
    };
    let expect = |mods| UiEvent::Click {
        node: NODE,
        row: 2,
        cell: 6,
        mods,
    };
    assert_eq!(click(&mut w, 0), [expect(0)]);
    assert_eq!(click(&mut w, 2), [expect(2)]);
    // A click in the gutter is cell 0.
    let gutter = Point::new(CODE_PAD + cw, CODE_PAD + rh);
    let out = w.input(
        Input::Button {
            at: gutter,
            down: true,
            mods: Mods(0),
        },
        &M,
    );
    assert_eq!(
        out,
        [UiEvent::Click {
            node: NODE,
            row: 1,
            cell: 0,
            mods: 0
        }]
    );
}

#[test]
fn the_wheel_over_a_code_view_scrolls_it_in_rows() {
    let mut w = window(sample());
    w.commit(&M);
    let out = w.input(
        Input::Wheel {
            at: Point::new(100, 100),
            dy: -2,
        },
        &M,
    );
    assert_eq!(
        out,
        [UiEvent::Scroll {
            node: NODE,
            rows: -6
        }]
    );
}

#[test]
fn an_app_is_held_to_the_limits() {
    let put = |v: CodeView| {
        let mut w = Window::new("Editor", 400, 300);
        w.put(NODE, ROOT, Layout::FILL, Widget::Code(v))
    };
    assert_eq!(put(view(vec![row("ok", "1"); MAX_CODE_ROWS])), Ok(()));
    assert_eq!(
        put(view(vec![row("x", ""); MAX_CODE_ROWS + 1])),
        Err(UiError::TooBig)
    );
    assert_eq!(
        put(view(vec![row(&"x".repeat(MAX_CODE_CELLS + 1), "")])),
        Err(UiError::TooBig)
    );
    let mut spans = view(vec![row("abc", "")]);
    spans.rows[0].spans = vec![CodeSpan {
        from: 2,
        to: 2,
        tone: Tone::Normal,
        link: false,
    }];
    assert_eq!(put(spans), Err(UiError::TooBig), "an empty span");
    let mut many = view(vec![row("abc", "")]);
    many.rows[0].spans = (0..33)
        .map(|i| CodeSpan {
            from: i,
            to: i + 1,
            tone: Tone::Normal,
            link: false,
        })
        .collect();
    assert_eq!(put(many), Err(UiError::TooBig), "too many spans");
}

#[test]
fn a_hostile_view_draws_without_panicking() {
    // Spans outside the text, overlapping, backwards-sorted; a cursor far away.
    let mut v = view(vec![row("héllo wörld", "é")]);
    v.rows[0].spans = vec![
        CodeSpan {
            from: 8,
            to: 300,
            tone: Tone::Error,
            link: true,
        },
        CodeSpan {
            from: 2,
            to: 9,
            tone: Tone::Ok,
            link: false,
        },
    ];
    v.cursor = Some((u32::MAX, u32::MAX));
    v.selected = Some(u32::MAX);
    v.gutter_width = u16::MAX;
    let mut w = window(v);
    let frame = w.commit(&M);
    assert!(!frame.list.groups.is_empty());
}
