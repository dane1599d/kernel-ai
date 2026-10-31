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

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;

use alloc::string::String;
use alloc::vec::Vec;
use comp_files::calls::Call;
use comp_files::names::path_ok;
use comp_files::{Files, Os};
use comp_ui_bind::{OnUi, Shown, Sink};
use compsdk::app::{Effect, Event, Events};
use compsdk::{Component, ImportError};
use minicbor::{Decode, Encode};
use uikit::widget::{UiError, UiEvent};

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// The window's title.
pub const TITLE: &str = "Files";

/// What the checkpoint keeps: the folder the window shows, so a restart comes
/// back to it. Everything else is read again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V1 {
    #[n(0)]
    pub dir: String,
}

/// The effect queue: `kai:app/effects`.
pub struct Effects;

impl Os for Effects {
    fn request(&mut self, call: &Call) -> u32 {
        compsdk::app::request(Effect::Call {
            name: String::from(call.name),
            args: call.args.clone(),
        })
    }
}

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
            Event::EffectDone { ticket, outcome } => {
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

    /// The folder the last version showed; anything else is home.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        let v1: V1 = compsdk::state::decode(state, SCHEMA).ok_or(ImportError::Bad)?;
        let dir = if path_ok(&v1.dir) {
            v1.dir
        } else {
            String::new()
        };
        let mut app = Self::new(Files::at(&dir));
        app.present_logged();
        Ok(app)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| {
            let v1 = V1 {
                dir: String::from(self.files.dir()),
            };
            compsdk::state::encode(SCHEMA, &v1)
        })
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
