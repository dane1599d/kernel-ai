//! The editor's interfaces on the host's side of the component boundary:
//! `app:editor/actions@1.0.0` (`apps/editor/wit/editor.wit`), the granted
//! `kai:edit@0.1.0` and the code view `kai:code@0.1.0`, beside the `kai:ui`
//! window the shared `comp-ui-bind` binds. Each export reaches the one instance
//! through `compsdk::with`.

use crate::{EditorWindow, Host};
use alloc::string::String;
use alloc::vec::Vec;
use comp_ui_bind::Sink;
use comp_ui_bind::bind::View;
use comp_ui_bind::tree::Entry;
use uikit::style::Tone;
use uikit::widget::{self as w, NodeId, UiError};

mod actions {
    wit_bindgen::generate!({
        world: "app:editor/actions-only",
        path: ["../../wit/kai-live", "../../wit/kai-app", "../editor/wit"],
    });
    pub use exports::app::editor::actions::Guest;
}

mod edit {
    wit_bindgen::generate!({
        world: "kai:edit/editing",
        path: ["../../wit/kai-edit"],
    });
    pub use exports::kai::edit::replies::Guest;
    pub use kai::edit::edit::send;
}

mod code {
    wit_bindgen::generate!({
        world: "kai:code/coding",
        path: ["../../wit/kai-code"],
    });
    pub use exports::kai::code::code_input::{ClickEvent, Grid, Guest, KeyEvent};
    pub use kai::code::code::{CodeView, Row, Span, put};
    pub use kai::code::types as t;
}

/// The OS as the editor sees it: `kai:ui/view` and `kai:code/code` to draw with,
/// `kai:edit/edit` to reach the edit service, the call's clock.
pub struct Os;

fn error(e: code::t::CodeError) -> UiError {
    match e {
        code::t::CodeError::NoWindow => UiError::NoWindow,
        code::t::CodeError::NoNode => UiError::NoNode,
        code::t::CodeError::BadParent => UiError::BadParent,
        code::t::CodeError::TooBig => UiError::TooBig,
        code::t::CodeError::TooManyWindows => UiError::TooManyWindows,
    }
}

fn extent(s: w::Size) -> code::t::Extent {
    match s {
        w::Size::Fit => code::t::Extent::Fit,
        w::Size::Fill(n) => code::t::Extent::Fill(n),
        w::Size::Fixed(n) => code::t::Extent::Fixed(n),
    }
}

fn layout(l: &w::Layout) -> code::t::Layout {
    let p = l.padding;
    code::t::Layout {
        width: extent(l.width),
        height: extent(l.height),
        padding: code::t::Insets {
            top: p.top,
            right: p.right,
            bottom: p.bottom,
            left: p.left,
        },
        gap: l.gap,
    }
}

fn tone(t: Tone) -> code::t::Tone {
    match t {
        Tone::Normal => code::t::Tone::Normal,
        Tone::Muted => code::t::Tone::Muted,
        Tone::Dim => code::t::Tone::Dim,
        Tone::Accent => code::t::Tone::Accent,
        Tone::Ok => code::t::Tone::Ok,
        Tone::Error => code::t::Tone::Error,
    }
}

fn view(v: &w::CodeView) -> code::CodeView {
    code::CodeView {
        label: v.label.clone(),
        gutter_width: v.gutter_width,
        rows: v
            .rows
            .iter()
            .map(|r| code::Row {
                text: r.text.clone(),
                gutter: r.gutter.clone(),
                spans: r
                    .spans
                    .iter()
                    .map(|s| code::Span {
                        start: s.from,
                        end: s.to,
                        tone: tone(s.tone),
                        link: s.link,
                    })
                    .collect(),
            })
            .collect(),
        cursor: v.cursor,
        selected: v.selected,
    }
}

impl Sink for Os {
    fn open(&mut self, title: &str, width: u16, height: u16) -> Result<u32, UiError> {
        View.open(title, width, height)
    }

    fn put(&mut self, window: u32, e: &Entry) -> Result<(), UiError> {
        match &e.widget {
            w::Widget::Code(v) => {
                code::put(window, e.id, e.parent, layout(&e.layout), &view(v)).map_err(error)
            }
            _ => View.put(window, e),
        }
    }

    fn remove(&mut self, window: u32, node: NodeId) -> Result<(), UiError> {
        View.remove(window, node)
    }

    fn commit(&mut self, window: u32) -> Result<(), UiError> {
        View.commit(window)
    }

    fn close(&mut self, window: u32) -> Result<(), UiError> {
        View.close(window)
    }
}

impl Host for Os {
    fn send(&mut self, message: &[u8]) -> u32 {
        edit::send(message)
    }

    fn now_ms(&self) -> u64 {
        compsdk::app::now_ms()
    }
}

struct Bound;

fn started<R>(f: impl FnOnce(&mut EditorWindow) -> R) -> Option<R> {
    compsdk::with::<EditorWindow, R>(f)
}

fn run(f: impl FnOnce(&mut EditorWindow, &mut Os) -> Result<(), UiError>) {
    if let Some(Err(e)) = started(|app| f(app, &mut Os)) {
        compsdk::log(&alloc::format!("editor window: {e:?}"));
    }
}

impl actions::Guest for Bound {
    fn open(component: String, path: String, line: u32) -> Result<String, String> {
        match started(|app| app.act_open(&mut Os, &component, &path, line)) {
            Some(result) => result,
            None => panic!("the editor was called before it started"),
        }
    }

    fn status() -> String {
        match started(|app| app.status(&mut Os)) {
            Some(text) => text,
            None => panic!("the editor was called before it started"),
        }
    }
}

impl edit::Guest for Bound {
    fn on_reply(ticket: u32, outcome: Result<Vec<u8>, String>) {
        run(|app, os| app.on_reply(os, ticket, outcome));
    }
}

impl code::Guest for Bound {
    fn on_key(_window: u32, _node: u32, e: code::KeyEvent) {
        run(|app, os| app.on_key(os, &e.name, &e.text, e.mods));
    }

    fn on_click(_window: u32, _node: u32, e: code::ClickEvent) {
        run(|app, os| app.on_click(os, e.row, e.cell, e.mods));
    }

    fn on_scroll(_window: u32, _node: u32, rows: i32) {
        run(|app, os| app.on_scroll(os, rows));
    }

    fn on_size(_window: u32, _node: u32, g: code::Grid) {
        run(|app, os| app.on_size(os, g.rows, g.cells));
    }
}

actions::export!(Bound with_types_in actions);
edit::export!(Bound with_types_in edit);
code::export!(Bound with_types_in code);
