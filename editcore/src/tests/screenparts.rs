//! The screen's smaller parts on their own: links, the pager, tabs and
//! scrolling sideways.

use crate::screen::input::Key;
use crate::screen::link::{self, Link};
use crate::screen::pager::{Act, Kind, Pager};
use crate::screen::render::{TAB, cell_of, col_of_cell};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

fn places(text: &str) -> Vec<(String, u32, u32, &str)> {
    link::scan(text)
        .into_iter()
        .map(|l| (l.path.clone(), l.line, l.column, &text[l.start..l.end]))
        .collect()
}

#[test]
fn links_are_found_in_the_places_logs_and_find_put_them() {
    assert_eq!(
        places(" --> components/notes/src/lib.rs:58:9"),
        [(
            "components/notes/src/lib.rs".into(),
            58,
            9,
            "components/notes/src/lib.rs:58:9"
        )]
    );
    assert_eq!(
        places("panicked at components/notes/src/lib.rs:58:9"),
        [(
            "components/notes/src/lib.rs".into(),
            58,
            9,
            "components/notes/src/lib.rs:58:9"
        )]
    );
    assert_eq!(
        places("src/ui.rs:7: let x = 1;"),
        [("src/ui.rs".into(), 7, 0, "src/ui.rs:7")]
    );
    assert_eq!(
        places("(lib.rs:12)"),
        [("lib.rs".into(), 12, 0, "lib.rs:12")]
    );
    assert_eq!(
        places("see a.rs:1, b.rs:2."),
        [
            ("a.rs".into(), 1, 0, "a.rs:1"),
            ("b.rs".into(), 2, 0, "b.rs:2"),
        ]
    );
}

#[test]
fn what_is_not_a_plain_source_place_is_not_a_link() {
    for text in [
        "std::io::Error:5",
        "/etc/passwd.txt:3",
        "../up/lib.rs:3",
        "a/../b.rs:3",
        "C:/x/lib.rs:3x",
        "lib.rs:0",
        "lib.rs:100000",
        "lib.rs:abc",
        "lib.rs",
        "lib.rs:",
        "noextension:5",
        ".rs:5",
        "version 1.2:5",
        "lib.rs:5x",
        &format!("{}.rs:5", "p".repeat(80)),
    ] {
        assert!(
            link::scan(text).is_empty(),
            "{text:?} {:?}",
            link::scan(text)
        );
    }
}

#[test]
fn a_column_finds_the_link_it_is_inside() {
    let text = "x lib.rs:12:3 y";
    let found: Option<Link> = link::at_column(text, 5);
    assert_eq!(found.map(|l| l.line), Some(12));
    assert!(link::at_column(text, 0).is_none());
    assert!(link::at_column(text, 14).is_none());
    assert!(link::at_column(text, 999).is_none());
}

#[test]
fn the_pager_scrolls_selects_and_follows_links() {
    let text = "3 hits\nsrc/a.rs:3: one\nsrc/b.rs:9: two\nno link here\nsrc/c.rs:1: four\nlast";
    let mut pager = Pager::new("find", Kind::Text, text);
    assert_eq!((pager.len(), pager.selected(), pager.top()), (6, 0, 0));
    assert_eq!(pager.key(Key::Down, 3), Act::None);
    let Act::Follow(link) = pager.key(Key::Enter, 3) else {
        panic!("a link on the row");
    };
    assert_eq!((link.path.as_str(), link.line), ("src/a.rs", 3));
    pager.key(Key::Down, 3);
    pager.key(Key::Down, 3);
    assert_eq!(pager.key(Key::Enter, 3), Act::None, "a row without a link");
    pager.key(Key::Down, 3);
    pager.key(Key::Down, 3);
    pager.key(Key::Down, 3);
    assert_eq!(pager.selected(), 5, "stops at the last row");
    assert_eq!(pager.top(), 3, "the page follows the selection");
    assert_eq!(pager.key(Key::Home, 3), Act::None);
    assert_eq!((pager.selected(), pager.top()), (0, 0));
    assert_eq!(pager.key(Key::End, 3), Act::None);
    assert_eq!(pager.selected(), 5);
    assert_eq!(pager.key(Key::PageUp, 3), Act::None);
    assert_eq!(pager.selected(), 3);
    for leave in [Key::Esc, Key::Char('q')] {
        assert_eq!(pager.key(leave, 3), Act::Close);
    }
}

#[test]
fn a_ctrl_click_follows_the_link_under_it_and_a_plain_click_only_selects() {
    let text = "head\nsrc/a.rs:3: one\nsrc/b.rs:9: two";
    let mut pager = Pager::new("find", Kind::Text, text);
    assert_eq!(pager.click(2, 3, false, 5), Act::None);
    assert_eq!(pager.selected(), 2);
    let Act::Follow(link) = pager.click(2, 3, true, 5) else {
        panic!("on the link");
    };
    assert_eq!(link.line, 9);
    assert_eq!(pager.click(2, 14, true, 5), Act::None, "beside the link");
    assert_eq!(pager.click(40, 0, true, 5), Act::None, "past the last row");
}

#[test]
fn the_files_pager_opens_the_file_on_the_row() {
    let text = "2 files; Enter opens one\n*> components/notes/src/lib.rs (120 lines)\n   components/notes/src/ui.rs (40 lines)";
    let mut pager = Pager::new("files", Kind::Files, text);
    assert_eq!(pager.selected(), 1, "the first file, not the heading");
    assert_eq!(
        pager.key(Key::Enter, 5),
        Act::Open("components/notes/src/lib.rs".into())
    );
    pager.key(Key::Down, 5);
    assert_eq!(
        pager.key(Key::Enter, 5),
        Act::Open("components/notes/src/ui.rs".into())
    );
}

#[test]
fn a_pager_cuts_what_it_is_given() {
    let many = (0..5000)
        .map(|n| format!("{n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let pager = Pager::new("big", Kind::Text, &many);
    assert_eq!(pager.len(), crate::screen::pager::MAX_LINES + 1);
    assert!(
        pager
            .line(crate::screen::pager::MAX_LINES)
            .unwrap()
            .contains("not shown")
    );
    let wide = Pager::new("wide", Kind::Text, &"w".repeat(10_000));
    assert_eq!(wide.line(0).unwrap().len(), crate::screen::pager::MAX_CHARS);
}

#[test]
fn tabs_spread_to_stops_and_clicks_map_back_to_characters() {
    assert_eq!(cell_of("\tx", 1), TAB);
    assert_eq!(cell_of("ab\tx", 3), TAB);
    assert_eq!(cell_of("abcd\tx", 5), 2 * TAB);
    assert_eq!(col_of_cell("\tx", 0), 0);
    assert_eq!(col_of_cell("\tx", 3), 0, "inside the tab");
    assert_eq!(col_of_cell("\tx", 4), 1);
    assert_eq!(col_of_cell("\tx", 99), 2, "past the end is the end");
    assert_eq!(col_of_cell("h\u{e9}llo", 2), 2);
}

#[test]
fn a_long_line_scrolls_sideways_so_the_cursor_shows() {
    use super::screen::Rig;
    let long = "0123456789".repeat(8);
    let mut rig = Rig::new(&format!("{long}\nshort\n"), 4, 24);
    // 24 cells, a gutter of 2 (one digit and a space): 22 cells of text.
    assert_eq!(rig.screen.view().rows[0].text, &long[..22]);
    rig.press("<end>");
    let view = rig.screen.view();
    let (row, cell) = view.cursor.unwrap();
    assert_eq!(row, 0);
    assert!(cell < 22, "the cursor stays inside the text area: {cell}");
    assert!(view.rows[0].text.ends_with("789"));
    rig.press("<home>");
    assert_eq!(rig.screen.view().rows[0].text, &long[..22]);
}

#[test]
fn links_in_text_rows_are_marked_by_cell() {
    use super::screen::Rig;
    let rig = Rig::new("// see src/ui.rs:5 for it\nplain\n", 4, 40);
    let view = rig.screen.view();
    assert_eq!(view.rows[0].links, [(7, 18)]);
    assert!(view.rows[1].links.is_empty());
}

#[test]
fn a_ctrl_click_on_a_link_in_the_text_goes_to_that_file() {
    use super::screen::Rig;
    let mut rig = Rig::new("// see lib.rs:3 here\na\nb\nc\n", 6, 40);
    rig.press("<down><down><down>");
    assert_eq!(rig.screen.line, 4);
    let gutter = rig.screen.view().gutter;
    rig.screen.input(crate::screen::Input::Click {
        row: 0,
        col: gutter + 8,
        ctrl: true,
    });
    rig.pump();
    assert_eq!(rig.screen.line, 3);
    // A click without Ctrl only moves the cursor.
    rig.screen.input(crate::screen::Input::Click {
        row: 1,
        col: gutter + 3,
        ctrl: false,
    });
    assert_eq!((rig.screen.line, rig.screen.col), (2, 1));
}
