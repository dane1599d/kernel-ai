//! The editor in the OS (P4 task 11): [`EditorWindow`] is `editcore::screen`'s
//! `Screen` with the window `comp-editor-ui` builds from it. Keys, clicks and
//! the wheel arrive from the code view (`kai:code/code-input`), become the
//! screen's input, and what the screen asks of the edit service leaves as
//! `kai:edit/edit.send`; the service's answer comes back in a later call
//! (`kai:edit/replies.on-reply`) and goes to the screen. So nothing here waits.
//!
//! A build runs on the host in the background. The screen polls its status when
//! it is next given something to do, which is the next key, click or answer: the
//! app has no timer, so a build that finishes while nobody types is shown at the
//! next touch.
//!
//! The agent's two actions only move the editor and say where it is; an edit is
//! typed, and a build starts from a key or a click in the window, never from an
//! action (comphost's build gate, on `wasmhost`'s `user` flag).

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;
pub mod keys;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_editor_ui as ui;
use comp_ui_bind::{OnUi, Shown, Sink};
use compsdk::{Component, ImportError};
use editcore::screen::{Input, Key, Screen};
use minicbor::{Decode, Encode};
use uikit::widget::{UiError, UiEvent};

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// Rows and cells the screen assumes until the code view says what shows.
const START: (usize, usize) = (24, 80);
/// Most requests sent and not answered.
const MAX_PENDING: usize = 8;

/// What the checkpoint keeps. Nothing yet: the session lives in the edit
/// service, and a draft in storage is a later step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V1 {
    #[n(0)]
    pub reserved: u32,
}

/// What the editor needs of the OS besides drawing: `kai:edit/edit.send`, and the
/// call's clock.
pub trait Host: Sink {
    /// Queues `message` for the edit service; the ticket its answer comes under.
    fn send(&mut self, message: &[u8]) -> u32;
    fn now_ms(&self) -> u64;
}

pub struct EditorWindow {
    screen: Screen,
    shown: Shown,
    /// Requests sent and not answered: (ticket, the screen's number for it).
    tickets: Vec<(u32, u32)>,
}

impl EditorWindow {
    fn new() -> Self {
        Self {
            screen: Screen::new(START.0, START.1),
            shown: Shown::new(),
            tickets: Vec::new(),
        }
    }

    /// The open window's id.
    pub fn window(&self) -> Option<u32> {
        self.shown.id()
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Opens the window if it is closed and sends what changed.
    pub fn present(&mut self, host: &mut dyn Host) -> Result<(), UiError> {
        let tree = ui::build(&self.screen.view());
        self.shown.present(host, ui::WINDOW_TITLE, ui::SIZE, tree)
    }

    /// Sends what the screen wants sent; it has one request in flight at a time.
    fn pump(&mut self, host: &mut dyn Host) {
        while self.tickets.len() < MAX_PENDING {
            let Some(out) = self.screen.next_request() else {
                break;
            };
            let ticket = host.send(&out.request.encode());
            self.tickets.push((ticket, out.id));
        }
    }

    /// After anything that may have changed the screen: look at a running build,
    /// send what is wanted and draw.
    fn settle(&mut self, host: &mut dyn Host) -> Result<(), UiError> {
        self.screen.tick(host.now_ms());
        self.pump(host);
        self.present(host)
    }

    /// The edit service's answer to `ticket`, or why there is none.
    pub fn on_reply(
        &mut self,
        host: &mut dyn Host,
        ticket: u32,
        outcome: Result<Vec<u8>, String>,
    ) -> Result<(), UiError> {
        let Some(at) = self.tickets.iter().position(|(t, _)| *t == ticket) else {
            return Ok(());
        };
        let (_, id) = self.tickets.remove(at);
        match outcome {
            Ok(bytes) => self.screen.reply(id, &String::from_utf8_lossy(&bytes)),
            Err(why) => self.screen.failed(id, &why),
        }
        self.settle(host)
    }

    /// A key the code view had focus for.
    pub fn on_key(
        &mut self,
        host: &mut dyn Host,
        name: &str,
        text: &str,
        mods: u8,
    ) -> Result<(), UiError> {
        if let Some(input) = keys::key_input(name, text, mods) {
            self.screen.input(input);
        }
        self.settle(host)
    }

    /// A click at row `row` and text cell `cell` of the code view.
    pub fn on_click(
        &mut self,
        host: &mut dyn Host,
        row: u32,
        cell: u32,
        mods: u8,
    ) -> Result<(), UiError> {
        let gutter = self.screen.view().gutter;
        self.screen
            .input(keys::click_input(row, cell, mods, gutter));
        self.settle(host)
    }

    /// The wheel turned `rows` over the code view: the cursor moves that far.
    pub fn on_scroll(&mut self, host: &mut dyn Host, rows: i32) -> Result<(), UiError> {
        let key = if rows < 0 { Key::Up } else { Key::Down };
        for _ in 0..rows.unsigned_abs().min(64) {
            self.screen.input(Input::Key(key));
        }
        self.settle(host)
    }

    /// The code view shows `rows` rows of `cells` text cells.
    pub fn on_size(&mut self, host: &mut dyn Host, rows: u16, cells: u16) -> Result<(), UiError> {
        let gutter = self.screen.view().gutter;
        self.screen
            .resize(usize::from(rows).max(1), usize::from(cells) + gutter);
        self.settle(host)
    }

    /// `editor_open`: the component's source at a line. The service's answer
    /// comes later; the window shows it then.
    pub fn act_open(
        &mut self,
        host: &mut dyn Host,
        component: &str,
        path: &str,
        line: u32,
    ) -> Result<String, String> {
        let plain = |s: &str| !s.is_empty() && s.is_ascii() && !s.chars().any(char::is_control);
        if !plain(component) || component.len() > 24 || component.contains(' ') {
            return Err(String::from("Give the component's name, such as notes."));
        }
        if !plain(path) || path.len() > 64 || path.contains(' ') || path.starts_with('/') {
            return Err(String::from("Give a path inside the component's source."));
        }
        if line == 0 || line > 99_999 {
            return Err(String::from("Lines count from 1."));
        }
        self.screen.open(component, None, Some((path, line)));
        self.settle(host)
            .map_err(|e| format!("the window: {e:?}"))?;
        Ok(format!("Opening {component} at {path}:{line}."))
    }

    /// `editor_status`: the file and line, the edits made and how the last build
    /// went, from the screen alone (it asks nobody).
    pub fn status(&mut self, host: &mut dyn Host) -> String {
        // A status poll for a running build is on the way after this call.
        let _ = self.settle(host);
        let view = self.screen.view();
        let (line, col) = self.screen.position();
        format!(
            "{}; line {line}, column {col}; {} edit(s){}; {}",
            view.title,
            self.screen.edits(),
            if self.screen.is_building() {
                "; building"
            } else {
                ""
            },
            if view.status.is_empty() {
                "no message"
            } else {
                &view.status
            },
        )
    }
}

impl OnUi for EditorWindow {
    /// The window has no `kai:ui` controls: only its closing.
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError> {
        if self.shown.id() == Some(window) && *event == UiEvent::CloseRequested {
            self.shown.closed(sink, window)?;
        }
        Ok(())
    }
}

impl Component for EditorWindow {
    const SCHEMA: u32 = SCHEMA;

    /// An empty window; the editor is opened on a file by `editor_open`.
    fn init() -> Self {
        let mut app = Self::new();
        app.present_logged();
        app
    }

    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        let _: V1 = compsdk::state::decode(state, SCHEMA).ok_or(ImportError::Bad)?;
        let mut app = Self::new();
        app.present_logged();
        Ok(app)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| compsdk::state::encode(SCHEMA, &V1::default()))
    }

    fn health(&self) -> u32 {
        0
    }

    fn self_test(&self) -> u32 {
        0
    }
}

impl EditorWindow {
    /// Draws the window, or logs why it could not.
    #[cfg(target_family = "wasm")]
    fn present_logged(&mut self) {
        if let Err(e) = self.present(&mut bind::Os) {
            compsdk::log(&format!("editor window: {e:?}"));
        }
    }

    /// There is no window on the host; tests draw through their own [`Host`].
    #[cfg(not(target_family = "wasm"))]
    fn present_logged(&mut self) {}
}

compsdk::component!(EditorWindow);
#[cfg(target_family = "wasm")]
comp_ui_bind::input!(EditorWindow);
