//! Trees the tests share: a Notes-shaped window and a page of every widget.

#![allow(dead_code)]

use uikit::Window;
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Insets, Layout, ROOT, Size, TextField, Widget};

pub fn s(t: &str) -> String {
    String::from(t)
}

pub fn field(label: &str, value: &str, placeholder: &str) -> TextField {
    TextField {
        label: s(label),
        value: s(value),
        placeholder: s(placeholder),
        max_len: 2000,
    }
}

pub fn label(text: &str) -> Widget {
    Widget::Label {
        text: s(text),
        style: TypeStyle::Body,
        tone: Tone::Normal,
    }
}

pub fn padded(gap: u16) -> Layout {
    Layout {
        padding: Insets::all(12),
        gap,
        ..Layout::FILL
    }
}

pub fn wide() -> Layout {
    Layout {
        width: Size::Fill(1),
        ..Layout::FIT
    }
}

/// Notes as P3 will show it: a list on the left, the note on the right.
pub fn notes() -> Window {
    let mut w = Window::new("Notes", 480, 300);
    w.put(
        1,
        ROOT,
        Layout::FILL,
        Widget::Split {
            axis: Axis::Row,
            first: 350,
        },
    )
    .unwrap();
    w.put(2, 1, padded(8), Widget::Stack { axis: Axis::Column })
        .unwrap();
    w.put(
        3,
        2,
        wide(),
        Widget::Button {
            text: s("New note"),
            action: Some(s("notes_add")),
        },
    )
    .unwrap();
    w.put(
        4,
        2,
        Layout::FILL,
        Widget::ItemList {
            label: s("Notes"),
            selected: Some(0),
        },
    )
    .unwrap();
    w.put(10, 4, wide(), label("Groceries")).unwrap();
    w.put(11, 4, wide(), label("Ideas")).unwrap();
    w.put(5, 1, padded(8), Widget::Stack { axis: Axis::Column })
        .unwrap();
    w.put(
        6,
        5,
        wide(),
        Widget::TextLine(field("Title", "Groceries", "Title")),
    )
    .unwrap();
    w.put(
        7,
        5,
        Layout::FILL,
        Widget::TextArea(field("", "milk\neggs", "Write here")),
    )
    .unwrap();
    w
}

/// One of each other widget, in a column.
pub fn gallery() -> Window {
    let mut w = Window::new("Gallery", 360, 400);
    w.put(1, ROOT, padded(10), Widget::Stack { axis: Axis::Column })
        .unwrap();
    w.put(
        2,
        1,
        Layout::FIT,
        Widget::Label {
            text: s("Settings"),
            style: TypeStyle::Heading,
            tone: Tone::Normal,
        },
    )
    .unwrap();
    w.put(
        3,
        1,
        Layout::FIT,
        Widget::Toggle {
            label: s("Wrap lines"),
            on: true,
        },
    )
    .unwrap();
    w.put(
        4,
        1,
        Layout::FIT,
        Widget::Slider {
            label: s("Size"),
            min: 10,
            max: 30,
            step: 2,
            value: 14,
        },
    )
    .unwrap();
    w.put(
        5,
        1,
        Layout::FIT,
        Widget::Progress {
            label: s("Indexing"),
            done: Some(500),
        },
    )
    .unwrap();
    w.put(
        6,
        1,
        Layout::FIT,
        Widget::Menu {
            label: s("Sort"),
            items: vec![s("Name"), s("Date")],
        },
    )
    .unwrap();
    w.put(
        7,
        1,
        wide(),
        Widget::Table {
            label: s("Files"),
            columns: vec![s("Name"), s("Size")],
            rows: vec![vec![s("a.txt"), s("12")], vec![s("b.txt"), s("3")]],
            selected: Some(1),
        },
    )
    .unwrap();
    w.put(
        8,
        1,
        Layout::FILL,
        Widget::Tabs {
            labels: vec![s("One"), s("Two")],
            selected: 0,
        },
    )
    .unwrap();
    w.put(9, 8, Layout::FIT, label("first page")).unwrap();
    w.put(12, 8, Layout::FIT, label("second page")).unwrap();
    w
}
