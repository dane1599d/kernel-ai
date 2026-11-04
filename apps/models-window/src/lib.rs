//! The model manager in the OS (P4 task 10, #556): [`ModelsWindow`] is the
//! model list (`comp-models`) with the window `comp-models-ui` builds from it.
//! It opens its window when it starts and asks the host for the list
//! (`kai:models`, granted to this app alone); each list that comes back is
//! drawn, and Refresh asks again.
//!
//! It queues no effect. Use asks the host to switch to the chosen model
//! (`use-model`), which the shell carries out through Kai's `model_use` row,
//! logged and undoable, but only when a person's key or click pressed it: the
//! agent pressing it through `ui_act` is answered with why not. What the shell
//! answered is the note under the table, and the list is read again. An import
//! is still shown as what to ask Kai (`comp_models_ui::ask_kai`), since it asks
//! on the trusted path, which a window cannot reach.
//!
//! When the window is closed the app stops reading, so nothing waits on the
//! host and the app can nap like any other. A list that arrives after that
//! draws nothing.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_models::{Intent, Models, Sha256, hex};
use comp_models_ui::{REFRESH, View};
use comp_ui_bind::{OnUi, Shown, Sink};
use compsdk::app::{Event, Events};
use compsdk::{Component, ImportError};
use minicbor::{Decode, Encode};
use uikit::widget::{UiError, UiEvent};

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// The window's title.
pub const TITLE: &str = "Models";

/// What the checkpoint keeps: the chosen model, so a restart comes back to it.
/// The list is read again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V1 {
    /// The chosen model's SHA-256 in hex, or empty.
    #[n(0)]
    pub selected: String,
}

pub struct ModelsWindow {
    pub models: Models,
    pub view: View,
    shown: Shown,
    /// Whether the window is still wanted: it is not once the user closed it.
    open: bool,
    /// Whether a read is out, asked for and not yet answered.
    reading: bool,
}

impl ModelsWindow {
    fn new(view: View) -> Self {
        Self {
            models: Models::default(),
            view: View {
                read_only: true,
                switches: true,
                ..view
            },
            shown: Shown::new(),
            open: true,
            reading: false,
        }
    }

    /// Draws the window through `sink`: opens it if it is closed.
    pub fn present(&mut self, sink: &mut dyn Sink) -> Result<(), UiError> {
        let tree = comp_models_ui::build(&self.models, &self.view);
        self.shown.present(sink, TITLE, comp_models_ui::SIZE, tree)
    }

    /// Draws the window, or logs why it could not; nothing once it was closed.
    #[cfg(target_family = "wasm")]
    fn present_logged(&mut self) {
        if !self.open {
            return;
        }
        if let Err(e) = self.present(&mut comp_ui_bind::bind::View) {
            compsdk::log(&format!("models window: {e:?}"));
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn present_logged(&mut self) {}

    /// The window's open id, if it is open.
    pub fn window(&self) -> Option<u32> {
        self.shown.id()
    }

    /// Whether a read of the list is out.
    pub fn reading(&self) -> bool {
        self.reading
    }

    /// Asks the host for the list, if the window is wanted and no read is out
    /// already.
    #[cfg(target_family = "wasm")]
    fn ask(&mut self) {
        if self.open && !self.reading {
            bind::read();
            self.reading = true;
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn ask(&mut self) {
        if self.open {
            self.reading = true;
        }
    }

    /// Asks the host to switch to `sha256` from the next restart. Whether the
    /// shell runs it is the host's and the shell's call: a press the person
    /// did not make is answered with why not.
    #[cfg(target_family = "wasm")]
    fn send_use(&mut self, sha256: &Sha256) {
        bind::use_model(&hex(sha256));
    }

    #[cfg(not(target_family = "wasm"))]
    fn send_use(&mut self, _sha256: &Sha256) {}

    /// What came of a switch: the shell's words, or why it did not run. The
    /// list is read again, so the table shows what the next restart uses.
    pub fn used(&mut self, outcome: Result<String, String>) {
        self.view.note = Some(comp_models_ui::switched(outcome));
        self.present_logged();
        self.ask();
    }

    /// A choice from the window: a switch is sent, and the note says it waits;
    /// an import is what to ask Kai.
    fn chose(&mut self, intent: &Intent) -> String {
        match intent {
            Intent::Use(sha256) => {
                self.send_use(sha256);
                comp_models_ui::switching(&self.models, sha256)
            }
            // An import has no undo, so a click alone never runs it. TODO: once
            // the shell can open the trusted Allow / Deny card itself (lane C's
            // install-card follow-up 2), an Import click may raise that card.
            Intent::Import(_) => comp_models_ui::ask_kai(&self.models, intent),
        }
    }

    /// A list came back for a read, or the reason there is none. What is not a
    /// list is one the window does not show, and it says so.
    pub fn listed(&mut self, outcome: Result<String, String>) {
        self.reading = false;
        let list = outcome.map(|text| modelchan::listing::parse(&text));
        self.view.note = match list {
            Ok(Ok(models)) => {
                self.models = models;
                None
            }
            Ok(Err(_)) => Some(String::from(
                "The list came in a form this version does not know.",
            )),
            Err(why) => Some(format!("The list could not be read: {why}")),
        };
        self.present_logged();
    }
}

impl OnUi for ModelsWindow {
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError> {
        if self.shown.id() != Some(window) {
            return Ok(());
        }
        match event {
            UiEvent::CloseRequested => {
                self.open = false;
                return self.shown.closed(sink, window).map(|_| ());
            }
            UiEvent::Pressed(REFRESH) => {
                self.ask();
                return Ok(());
            }
            _ => {}
        }
        let (redraw, intent) = comp_models_ui::on_ui(&self.models, &mut self.view, event);
        let said = intent.map(|i| self.chose(&i));
        let told = said.is_some();
        if told {
            self.view.note = said;
        }
        match redraw || told {
            true => self.present(sink),
            false => Ok(()),
        }
    }
}

impl Events for ModelsWindow {
    /// Nothing queues an effect, so there is no answer to wait for; `started`
    /// needs nothing either (the first read was asked for in `init`).
    fn on_event(&mut self, _event: Event) {}
}

impl Component for ModelsWindow {
    const SCHEMA: u32 = SCHEMA;

    /// An empty window, and the first read; the models come with the host's
    /// answer.
    fn init() -> Self {
        let mut app = Self::new(View::default());
        app.present_logged();
        app.ask();
        app
    }

    /// The model the last version had chosen; anything else starts as new.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        let v1: V1 = compsdk::state::decode(state, SCHEMA).ok_or(ImportError::Bad)?;
        let selected = modelchan::listing::sha256(&v1.selected).ok();
        let mut app = Self::new(View {
            selected,
            ..View::default()
        });
        app.present_logged();
        app.ask();
        Ok(app)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| {
            let v1 = V1 {
                selected: self.view.selected.map(|s| hex(&s)).unwrap_or_default(),
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

compsdk::component!(ModelsWindow);
compsdk::events!(ModelsWindow);
#[cfg(target_family = "wasm")]
comp_ui_bind::input!(ModelsWindow);
