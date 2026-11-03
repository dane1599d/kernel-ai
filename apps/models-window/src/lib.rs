//! The model manager in the OS (P4 task 10), read-only for v0.1:
//! [`ModelsWindow`] is the model list (`comp-models`) with the window
//! `comp-models-ui` builds from it. It opens its window when it starts and asks
//! the host for the list (`kai:models`, granted to this app alone); each list
//! that comes back is drawn, and Refresh asks again.
//!
//! It changes nothing and queues no effect. A choice is shown as what to ask
//! Kai (`comp_models_ui::ask_kai`), whose `model_use` and `model_fetch` rows
//! are the one way to switch or import a model, logged and undoable. The
//! window carrying a choice out itself is #556, after v0.1.
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
use comp_models::{Models, hex};
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
        // A choice is never carried out here: it is what to ask Kai.
        let said = intent.map(|i| comp_models_ui::ask_kai(&self.models, &i));
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
