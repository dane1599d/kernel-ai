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
//! The agent's three actions only move the editor (to a component's source, or to
//! one of the person's files by path) and say where it is; an edit is typed, and a
//! save or a build starts from a key or a click in the window, never from an action
//! (comphost's write and build gates, on `wasmhost`'s `user` flag).
//!
//! The editor also opens a file of the person's own: Ctrl-O asks for its path and
//! Ctrl-S saves it, through the OS's file door ([`files`]). The file's lines are
//! then edited here and the edit service is not asked about them. A file in a C
//! component's folder, `/home/user/c/<name>/`, is built with Ctrl-B: the OS does
//! what a typed `cc build <name>` does, from the saved files, and the report
//! comes back to the status row and, for a failure, the pager.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;
mod files;
pub mod keys;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_editor_ui as ui;
use comp_ui_bind::{Shown, Sink};
use compsdk::{Component, ImportError};
use editcore::screen::{Input, Key, Screen};
use files::{Files, Kept};
use minicbor::{Decode, Encode};
use uikit::widget::{UiError, UiEvent};

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// Rows and cells the screen assumes until the code view says what shows.
const START: (usize, usize) = (24, 80);
/// Most requests sent and not answered.
const MAX_PENDING: usize = 8;
/// What an editor with nothing open says, so a person who opens it knows what to do.
const EMPTY: &str = "No file is open: Ctrl-O opens one by its path.";

/// What the checkpoint keeps of the file of the person's own that is open, so
/// an editor that napped or came back from a swap has it again: its path, to read
/// it from the disk, and the text as it stood when that is not what the disk has,
/// with the line being typed in it, to put back over what is read. The text is at
/// most [`files::KEEP_MAX`] (a checkpoint is at most 64 KiB); past it, which the
/// editor's own limit keeps from happening, only the path is kept. A component's
/// source is in the edit service, which keeps its session. A window with text
/// not saved also holds a nap lease (comphost, which caps it) and says so
/// (`set-unsaved`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V1 {
    #[n(0)]
    pub reserved: u32,
    /// Absent in a checkpoint an earlier version saved.
    #[n(1)]
    pub path: Option<String>,
    /// The file's text as it stood, when it differed from the disk's: absent
    /// when nothing was unsaved, and in a checkpoint an earlier version saved.
    #[n(2)]
    pub text: Option<String>,
    /// The line the cursor was on, from 1.
    #[n(3)]
    pub line: Option<u32>,
}

/// Where the window is in being closed with changes not saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Closing {
    /// Not being closed.
    No,
    /// Asked to close with changes not saved: the window says so and waits for
    /// Save, Discard or Cancel.
    Asking,
    /// Save was chosen: the window closes when the file is written.
    Saving,
}

/// What the editor needs of the OS besides drawing: `kai:edit/edit.send`, the
/// file door (`kai:files`) and the call's clock.
pub trait Host: Sink {
    /// Queues `message` for the edit service; the ticket its answer comes under.
    fn send(&mut self, message: &[u8]) -> u32;
    fn now_ms(&self) -> u64;
    /// Queues a read of the whole file at `path` (`kai:files`): the ticket the
    /// file's bytes, or why not, come back under.
    fn read_file(&mut self, path: &str) -> u32;
    /// Queues `data` as the whole text of the file at `path`, made when `create`
    /// and replaced otherwise: the ticket the answer comes back under.
    fn write_file(&mut self, path: &str, data: &[u8], create: bool) -> u32;
    /// Queues a build of the C component `name` from its saved sources
    /// (`kai:files`): the ticket the report, as text, comes back under.
    fn build_component(&mut self, name: &str) -> u32;
}

pub struct EditorWindow {
    screen: Screen,
    shown: Shown,
    /// Requests sent and not answered: (ticket, the screen's number for it).
    tickets: Vec<(u32, u32)>,
    /// The file of the person's own that is open, if any, and its transfer.
    files: Files,
    /// The screen's rows and columns as the code view last said, for a new screen.
    grid: (usize, usize),
    closing: Closing,
    /// The file a checkpoint had open and what it kept of it, still to be read
    /// again. `import` cannot do it: the host takes a `kai:files` request only from
    /// a call that a transaction wraps, and a migration is not one. The first call
    /// that is one reads it ([`EditorWindow::resume`]).
    reopen: Option<(String, Kept)>,
}

impl EditorWindow {
    fn new() -> Self {
        let mut screen = Screen::new(START.0, START.1);
        screen.note(EMPTY, false);
        Self {
            screen,
            shown: Shown::new(),
            tickets: Vec::new(),
            files: Files::default(),
            grid: START,
            closing: Closing::No,
            reopen: None,
        }
    }

    /// The open window's id.
    pub fn window(&self) -> Option<u32> {
        self.shown.id()
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Opens the window if it is closed and sends what changed: the tree, with the
    /// question when the window waits to be told whether to close, and whether
    /// the window holds changes not saved.
    pub fn present(&mut self, host: &mut dyn Host) -> Result<(), UiError> {
        let view = self.screen.view();
        let tree = match self.closing {
            Closing::Asking => ui::build_closing(&view, self.files.can_save()),
            _ => ui::build(&view),
        };
        let unsaved = self.files.unsaved(&self.screen);
        self.shown.mark_unsaved(unsaved);
        self.shown.present(host, ui::WINDOW_TITLE, ui::SIZE, tree)
    }

    /// Sends what the screen wants sent; it has one request in flight at a time.
    /// With a file open the file answers, in this call.
    fn pump(&mut self, host: &mut dyn Host) {
        if self.files.answer(&mut self.screen) {
            return;
        }
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
        self.resume(host);
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
        if self.files.owns(ticket) {
            self.files.reply(&mut self.screen, self.grid, outcome);
            // Saved, so closed as asked; not saved, the window stays and says why.
            if self.closing == Closing::Saving {
                self.closing = Closing::No;
                if !self.files.unsaved(&self.screen) {
                    return self.close_window(host);
                }
            }
            return self.settle(host);
        }
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
        self.resume(host);
        if self.closing != Closing::No {
            return self.closing_key(host, name, text, mods);
        }
        self.typed_key(host, name, text, mods)
    }

    /// A key for the screen: its input, what the screen asked for and what the
    /// file door is asked to do about it.
    fn typed_key(
        &mut self,
        host: &mut dyn Host,
        name: &str,
        text: &str,
        mods: u8,
    ) -> Result<(), UiError> {
        if let Some(input) = keys::key_input(name, text, mods) {
            self.screen.input(input);
        }
        let mut settled = self.settle(host);
        if self.take_file_wishes(host) {
            settled = settled.and(self.settle(host));
        }
        settled
    }

    /// Ctrl-O's path, Ctrl-S and Ctrl-B in a file, which the screen only notes: the
    /// file is the window's to open, save and build. Whether any was asked for.
    fn take_file_wishes(&mut self, host: &mut dyn Host) -> bool {
        let open = self.screen.take_open();
        let save = self.screen.take_save();
        let build = self.screen.take_build();
        if let Some(path) = &open
            && let Err(why) = self.files.ask_open(host, &mut self.screen, path)
        {
            self.screen.note(why, true);
        }
        if save {
            self.files.ask_save(host, &mut self.screen);
        }
        if build {
            self.files.ask_build(host, &mut self.screen);
        }
        open.is_some() || save || build
    }

    /// A click at row `row` and text cell `cell` of the code view.
    pub fn on_click(
        &mut self,
        host: &mut dyn Host,
        row: u32,
        cell: u32,
        mods: u8,
    ) -> Result<(), UiError> {
        self.resume(host);
        let gutter = self.screen.view().gutter;
        self.screen
            .input(keys::click_input(row, cell, mods, gutter));
        self.settle(host)
    }

    /// The wheel turned `rows` over the code view: the cursor moves that far.
    pub fn on_scroll(&mut self, host: &mut dyn Host, rows: i32) -> Result<(), UiError> {
        self.resume(host);
        let key = if rows < 0 { Key::Up } else { Key::Down };
        for _ in 0..rows.unsigned_abs().min(64) {
            self.screen.input(Input::Key(key));
        }
        self.settle(host)
    }

    /// The code view shows `rows` rows of `cells` text cells.
    pub fn on_size(&mut self, host: &mut dyn Host, rows: u16, cells: u16) -> Result<(), UiError> {
        let gutter = self.screen.view().gutter;
        self.grid = (usize::from(rows).max(1), usize::from(cells) + gutter);
        self.screen.resize(self.grid.0, self.grid.1);
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
        // The source the agent names takes the place of the file a nap kept.
        self.reopen = None;
        if self.files.unsaved(&self.screen) {
            return Err(String::from(
                "The editor holds edits that are not saved or built; save or reset them first.",
            ));
        }
        self.files.close(&mut self.screen, self.grid);
        self.screen.open(component, None, Some((path, line)));
        self.settle(host)
            .map_err(|e| format!("the window: {e:?}"))?;
        Ok(format!("Opening {component} at {path}:{line}."))
    }

    /// `editor_open_file`: one of the person's own files, by path. The OS reads it
    /// as it does for Ctrl-O, in a later call, and the window shows it then. The
    /// answer says only that it is on its way; the file's text never goes to the
    /// agent.
    pub fn act_open_file(&mut self, host: &mut dyn Host, path: &str) -> Result<String, String> {
        // The file the agent names takes the place of the one a nap kept.
        self.reopen = None;
        self.files.ask_open(host, &mut self.screen, path)?;
        self.settle(host)
            .map_err(|e| format!("the window: {e:?}"))?;
        Ok(format!("Opening {path}."))
    }

    /// A key while the window waits to be told whether to close with changes not
    /// saved: S saves, D discards, Esc keeps editing; the rest is not typed.
    fn closing_key(
        &mut self,
        host: &mut dyn Host,
        name: &str,
        text: &str,
        mods: u8,
    ) -> Result<(), UiError> {
        let plain = mods & (keys::CTRL | keys::ALT | keys::SUPER) == 0;
        match (name, text.to_ascii_lowercase().as_str()) {
            _ if self.closing == Closing::Saving => Ok(()),
            ("esc", _) => self.cancel_close(host),
            (_, "s") if plain && self.files.can_save() => self.save_and_close(host),
            (_, "d") if plain => self.discard_and_close(host),
            _ => Ok(()),
        }
    }

    /// The window is asked to close: at once when nothing is unsaved, and when
    /// something is, after the person says what to do with it.
    fn close_requested(&mut self, host: &mut dyn Host) -> Result<(), UiError> {
        if !self.files.unsaved(&self.screen) {
            return self.close_window(host);
        }
        self.closing = Closing::Asking;
        self.settle(host)
    }

    /// Closes the window, which the person or the question has finished with.
    fn close_window(&mut self, host: &mut dyn Host) -> Result<(), UiError> {
        self.closing = Closing::No;
        match self.shown.id() {
            Some(id) => self.shown.closed(host, id).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Save: writes the file and closes when it is written.
    fn save_and_close(&mut self, host: &mut dyn Host) -> Result<(), UiError> {
        self.closing = Closing::Saving;
        // Ctrl-S as typed: the screen lets go of the line it holds first.
        let asked = self.typed_key(host, "s", "", keys::CTRL);
        if !self.files.busy() {
            // The save was refused and the status says why: the window stays.
            self.closing = Closing::No;
            return asked.and(self.settle(host));
        }
        asked
    }

    /// Discard: the changes go with the window, and a window opened again is an
    /// editor with no file open.
    fn discard_and_close(&mut self, host: &mut dyn Host) -> Result<(), UiError> {
        self.files = Files::default();
        self.tickets.clear();
        self.screen = Screen::new(self.grid.0, self.grid.1);
        self.screen.note(EMPTY, false);
        self.close_window(host)
    }

    /// Cancel: back to editing.
    fn cancel_close(&mut self, host: &mut dyn Host) -> Result<(), UiError> {
        self.closing = Closing::No;
        self.settle(host)
    }

    /// An event from the window: its closing, and the three answers to the
    /// question a close with changes not saved asks.
    pub fn ui_event(
        &mut self,
        host: &mut dyn Host,
        window: u32,
        event: &UiEvent,
    ) -> Result<(), UiError> {
        if self.shown.id() != Some(window) {
            return Ok(());
        }
        match (event, self.closing) {
            (UiEvent::CloseRequested, Closing::No) => self.close_requested(host),
            (UiEvent::Pressed(ui::SAVE), Closing::Asking) if self.files.can_save() => {
                self.save_and_close(host)
            }
            (UiEvent::Pressed(ui::DISCARD), Closing::Asking) => self.discard_and_close(host),
            (UiEvent::Pressed(ui::CANCEL), Closing::Asking) => self.cancel_close(host),
            _ => Ok(()),
        }
    }

    /// Opens the file a checkpoint had open, on the first call that may ask the
    /// OS for a file; the reply comes in a later call, as for Ctrl-O's. A file that
    /// cannot be opened again is said in the status row, and the editor is empty.
    fn resume(&mut self, host: &mut dyn Host) {
        let Some((path, kept)) = self.reopen.take() else {
            return;
        };
        if let Err(why) = self.files.ask_restore(host, &mut self.screen, &path, kept) {
            self.screen
                .note(format!("{path} was not opened again: {why}"), true);
        }
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
        let kept: V1 = compsdk::state::decode(state, SCHEMA).ok_or(ImportError::Bad)?;
        let mut app = Self::new();
        app.reopen = kept.path.map(|path| {
            let line = kept.line.unwrap_or(1);
            (
                path,
                Kept {
                    text: kept.text,
                    line,
                },
            )
        });
        app.present_logged();
        Ok(app)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| compsdk::state::encode(SCHEMA, &self.checkpoint()))
    }

    fn health(&self) -> u32 {
        0
    }

    fn self_test(&self) -> u32 {
        0
    }
}

impl EditorWindow {
    /// What a checkpoint keeps. An editor that has only just come back and has
    /// not read its file yet keeps what it was given.
    fn checkpoint(&self) -> V1 {
        if let Some((path, kept)) = &self.reopen {
            return V1 {
                reserved: 0,
                path: Some(path.clone()),
                text: kept.text.clone(),
                line: Some(kept.line),
            };
        }
        let path = self.files.path().map(String::from);
        let text = self.files.unsaved_text(&self.screen);
        let line = path
            .as_ref()
            .and_then(|_| u32::try_from(self.screen.position().0).ok());
        V1 {
            reserved: 0,
            path,
            text,
            line,
        }
    }

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
