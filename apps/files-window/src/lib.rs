//! The Files app in the OS (P4 task 5): [`FilesWindow`] is `comp-files`'s model
//! with the window `comp-files-ui` builds from it. It opens its window when it
//! starts, redraws after each event, and asks the OS for everything through
//! effects: `fs_list`, `fs_read`, `fs_move`, `fs_trash` and `fs_restore` are the
//! agent core's rows, so their tiers, log and `undo` are the OS's. What the OS
//! answers comes back as `effect-done` events.
//!
//! Effects and the app's reply to them run in a transaction (`Host::serve_with`
//! for a window's input, a request for an action), so only calls can queue them:
//! the window opens in `init`, and the first listing waits for `started`.
//!
//! When the OS changes the person's files (the AI, the Terminal or an undo), it
//! tells the app with a notice (`effect-done` under `NOTICE_TICKET`, with the
//! text `NOTICE_FILES`), and the window lists its folder again: it holds
//! `fs_list` already, so the notice grants it nothing.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;

use alloc::string::String;
use alloc::vec::Vec;
use comp_files::calls::Call;
use comp_files::{Files, Os};
use comp_ui_bind::{OnUi, Shown, Sink};
use compsdk::app::{Effect, Event, Events, NOTICE_FILES, NOTICE_TICKET};
use compsdk::{Component, ImportError};
use minicbor::{Decode, Encode};
use uikit::widget::{UiError, UiEvent};

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// The window's title.
pub const TITLE: &str = "Files";

/// What the checkpoint keeps: nothing. The folder shown is only what is drawn,
/// and the host refuses a read-only row (`files_open`, `files_reveal`) that
/// changes the checkpoint, so it is not in it; everything is read again. A
/// restart or a version swap starts at the home folder. A state the last version
/// saved, with a folder in it, still imports: the folder is dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V1 {}

/// The effect queue: `kai:app/effects`.
pub struct Effects;

impl Os for Effects {
    fn request(&mut self, call: &Call) -> u32 {
        let ticket = compsdk::app::request(Effect::Call {
            name: String::from(call.name),
            args: call.args.clone(),
        });
        note(&alloc::format!(
            "files window: asked the OS for {} (ticket {ticket})",
            call.name
        ));
        ticket
    }
}

/// A line in the boot log of what the window asked the OS for and what came back,
/// so a window that never shows a folder says where it stopped.
#[cfg(target_family = "wasm")]
fn note(text: &str) {
    compsdk::log(text);
}

#[cfg(not(target_family = "wasm"))]
fn note(_text: &str) {}

pub struct FilesWindow {
    pub files: Files,
    shown: Shown,
}

impl FilesWindow {
    fn new(files: Files) -> Self {
        Self {
            files,
            shown: Shown::new(),
        }
    }

    /// Draws the window through `sink`: opens it if it is closed.
    pub fn present(&mut self, sink: &mut dyn Sink) -> Result<(), UiError> {
        let tree = comp_files_ui::build(&self.files);
        self.shown.present(sink, TITLE, comp_files_ui::SIZE, tree)
    }

    /// Draws the window, or logs why it could not.
    #[cfg(target_family = "wasm")]
    fn present_logged(&mut self) {
        if let Err(e) = self.present(&mut comp_ui_bind::bind::View) {
            compsdk::log(&alloc::format!("files window: {e:?}"));
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn present_logged(&mut self) {}

    /// The window's open id, if it is open.
    pub fn window(&self) -> Option<u32> {
        self.shown.id()
    }

    /// `files_open`, `files_reveal` and `files_select` run `f` on the model,
    /// then redraw to show the answer's first effect on the window.
    pub fn act(
        &mut self,
        f: impl FnOnce(&mut Files, &mut Effects) -> Result<String, String>,
    ) -> Result<String, String> {
        let said = f(&mut self.files, &mut Effects);
        self.present_logged();
        said
    }
}

impl OnUi for FilesWindow {
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError> {
        if self.shown.id() != Some(window) {
            return Ok(());
        }
        if *event == UiEvent::CloseRequested {
            return self.shown.closed(sink, window).map(|_| ());
        }
        match comp_files_ui::on_ui(&mut self.files, &mut Effects, event) {
            true => self.present(sink),
            false => Ok(()),
        }
    }
}

impl Events for FilesWindow {
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Started => self.files.refresh(&mut Effects),
            // The OS says the person's files changed (the AI, the Terminal or an
            // undo made, moved or trashed one): list the folder shown again,
            // keeping what is selected. No effect of ours has this ticket.
            Event::EffectDone {
                ticket: NOTICE_TICKET,
                outcome,
            } => match outcome.as_deref() {
                Ok(NOTICE_FILES) => {
                    note("files window: the files changed; listing the folder again");
                    self.files.refresh(&mut Effects);
                }
                _ => return,
            },
            Event::EffectDone { ticket, outcome } => {
                note(&alloc::format!(
                    "files window: ticket {ticket} came back {}",
                    if outcome.is_ok() { "ok" } else { "refused" }
                ));
                self.files.done(&mut Effects, ticket, outcome);
            }
        }
        self.present_logged();
    }
}

impl Component for FilesWindow {
    const SCHEMA: u32 = SCHEMA;

    /// A window with the home folder's name and nothing in it; the first
    /// listing is asked for when the OS says the app started.
    fn init() -> Self {
        let mut app = Self::new(Files::new());
        app.present_logged();
        app
    }

    /// A state it can read starts the window over, at home.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        let _: V1 = compsdk::state::decode(state, SCHEMA).ok_or(ImportError::Bad)?;
        let mut app = Self::new(Files::new());
        app.present_logged();
        Ok(app)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| compsdk::state::encode(SCHEMA, &V1 {}))
    }

    fn health(&self) -> u32 {
        0
    }

    fn self_test(&self) -> u32 {
        0
    }
}

compsdk::component!(FilesWindow);
compsdk::events!(FilesWindow);
#[cfg(target_family = "wasm")]
comp_ui_bind::input!(FilesWindow);
