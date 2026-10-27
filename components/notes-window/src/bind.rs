//! `kai:ui@0.1.0`'s bindings (`wit/window.wit`, world `window-only`): the
//! window's [`Sink`] on `kai:ui/view`, and `kai:ui/input`'s `on-ui` handed to
//! [`NotesWindow::event`].

use crate::{NotesWindow, Sink};
use comp_notes_ui::Entry;
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{self as w, Axis, NodeId, UiError, UiEvent, Value};

wit_bindgen::generate!({
    world: "app:notes-window/window-only",
    path: ["../../wit/kai-live", "../../wit/kai-app", "../../wit/kai-ui", "wit"],
    generate_all,
});

use exports::kai::ui::input::{self, Guest};
use kai::ui::types as t;
use kai::ui::view;

/// `kai:ui/view` itself.
pub struct View;

fn error(e: t::UiError) -> UiError {
    match e {
        t::UiError::NoWindow => UiError::NoWindow,
        t::UiError::NoNode => UiError::NoNode,
        t::UiError::BadParent => UiError::BadParent,
        t::UiError::TooBig => UiError::TooBig,
        t::UiError::TooManyWindows => UiError::TooManyWindows,
    }
}

impl Sink for View {
    fn open(&mut self, title: &str, width: u16, height: u16) -> Result<u32, UiError> {
        view::open(title, width, height).map_err(error)
    }

    fn put(&mut self, window: u32, e: &Entry) -> Result<(), UiError> {
        view::put(
            window,
            e.id,
            e.parent,
            layout(&e.layout),
            &widget(&e.widget),
        )
        .map_err(error)
    }

    fn remove(&mut self, window: u32, node: NodeId) -> Result<(), UiError> {
        view::remove(window, node).map_err(error)
    }

    fn commit(&mut self, window: u32) -> Result<(), UiError> {
        view::commit(window).map_err(error)
    }

    fn close(&mut self, window: u32) -> Result<(), UiError> {
        view::close(window).map_err(error)
    }
}

fn size(s: w::Size) -> t::Size {
    match s {
        w::Size::Fit => t::Size::Fit,
        w::Size::Fill(n) => t::Size::Fill(n),
        w::Size::Fixed(n) => t::Size::Fixed(n),
    }
}

fn layout(l: &w::Layout) -> t::Layout {
    let p = l.padding;
    t::Layout {
        width: size(l.width),
        height: size(l.height),
        padding: t::Insets {
            top: p.top,
            right: p.right,
            bottom: p.bottom,
            left: p.left,
        },
        gap: l.gap,
    }
}

fn axis(a: Axis) -> t::Axis {
    match a {
        Axis::Row => t::Axis::Row,
        Axis::Column => t::Axis::Column,
    }
}

/// `mono-title` is `desktop`'s; an app asking for it gets `mono`.
fn style(s: TypeStyle) -> t::TextStyle {
    match s {
        TypeStyle::Small => t::TextStyle::Small,
        TypeStyle::Body => t::TextStyle::Body,
        TypeStyle::Label => t::TextStyle::Label,
        TypeStyle::Heading => t::TextStyle::Heading,
        TypeStyle::Display => t::TextStyle::Display,
        TypeStyle::Mono | TypeStyle::MonoTitle => t::TextStyle::Mono,
    }
}

fn tone(x: Tone) -> t::Tone {
    match x {
        Tone::Normal => t::Tone::Normal,
        Tone::Muted => t::Tone::Muted,
        Tone::Dim => t::Tone::Dim,
        Tone::Accent => t::Tone::Accent,
        Tone::Ok => t::Tone::Ok,
        Tone::Error => t::Tone::Error,
    }
}

fn field(f: &w::TextField) -> t::TextField {
    t::TextField {
        label: f.label.clone(),
        value: f.value.clone(),
        placeholder: f.placeholder.clone(),
        max_len: f.max_len,
    }
}

fn widget(x: &w::Widget) -> t::Widget {
    use w::Widget as W;
    match x {
        W::Stack { axis: a } => t::Widget::Stack(t::Stack { axis: axis(*a) }),
        W::Label {
            text,
            style: s,
            tone: c,
        } => t::Widget::Label(t::Label {
            text: text.clone(),
            style: style(*s),
            tone: tone(*c),
        }),
        W::Button { text, action } => t::Widget::Button(t::Button {
            text: text.clone(),
            action: action.clone(),
        }),
        W::TextLine(f) => t::Widget::TextLine(field(f)),
        W::TextArea(f) => t::Widget::TextArea(field(f)),
        W::ItemList { label, selected } => t::Widget::ItemList(t::ItemList {
            label: label.clone(),
            selected: *selected,
        }),
        W::Table {
            label,
            columns,
            rows,
            selected,
        } => t::Widget::Table(t::Table {
            label: label.clone(),
            columns: columns.clone(),
            rows: rows.clone(),
            selected: *selected,
        }),
        W::Tabs { labels, selected } => t::Widget::Tabs(t::Tabs {
            labels: labels.clone(),
            selected: *selected,
        }),
        W::Toggle { label, on } => t::Widget::Toggle(t::Toggle {
            label: label.clone(),
            on: *on,
        }),
        W::Slider {
            label,
            min,
            max,
            step,
            value,
        } => t::Widget::Slider(t::Slider {
            label: label.clone(),
            min: *min,
            max: *max,
            step: *step,
            value: *value,
        }),
        W::Menu { label, items } => t::Widget::Menu(t::Menu {
            label: label.clone(),
            items: items.clone(),
        }),
        W::Scroll { axis: a } => t::Widget::Scroll(t::Scroll { axis: axis(*a) }),
        W::Split { axis: a, first } => t::Widget::Split(t::Split {
            axis: axis(*a),
            first: *first,
        }),
        W::Image { label, source } => t::Widget::Image(t::Image {
            label: label.clone(),
            source: source.clone(),
        }),
        W::Progress { label, done } => t::Widget::Progress(t::Progress {
            label: label.clone(),
            done: *done,
        }),
    }
}

fn event(e: input::UiEvent) -> UiEvent {
    match e {
        input::UiEvent::Pressed(node) => UiEvent::Pressed(node),
        input::UiEvent::Changed((node, v)) => UiEvent::Changed(
            node,
            match v {
                input::Value::Text(s) => Value::Text(s),
                input::Value::On(b) => Value::On(b),
                input::Value::Number(n) => Value::Number(n),
                input::Value::Index(i) => Value::Index(i),
            },
        ),
        input::UiEvent::Submitted(node) => UiEvent::Submitted(node),
        input::UiEvent::CloseRequested(_) => UiEvent::CloseRequested,
    }
}

struct Input;

impl Guest for Input {
    fn on_ui(window: u32, e: input::UiEvent) {
        let e = event(e);
        let done = compsdk::with::<NotesWindow, _>(|app| app.event(&mut View, window, &e));
        if let Some(Err(why)) = done {
            compsdk::log(&alloc::format!("notes window: {why:?}"));
        }
    }
}

export!(Input);
