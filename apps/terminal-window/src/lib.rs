//! The Terminal app in the OS (P4 task 4): [`TerminalWindow`] is `comp-terminal`'s
//! model with the window `comp-terminal-ui` builds from it. It opens its window
//! when it starts, redraws after each event, and sends every line, a person's
//! Enter or the AI's `terminal_run`, to the shell as one effect, `shell_line`.
//! What the shell answers comes back as an `effect-done` event.
//!
//! Effects and the app's reply to them run in a transaction (`Host::serve_with`
//! for a window's input, a request for an action), so only calls can queue
//! them: the window opens in `init` and nothing is asked for until a call.
//!
//! The window is presented here rather than by `comp_ui_bind::Shown`: the input
//! line keeps what a person typed in the window's field between trees, and the
//! scrollback has a row a line, so the diff must be quick and must know that.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;

use alloc::string::String;
use alloc::vec::Vec;
use comp_terminal::Terminal;
use comp_terminal::os::Os;
use comp_terminal::state::{self, State};
use comp_terminal_ui::{Change, Entry, View, next, on_ui};
use comp_ui_bind::tree;
use comp_ui_bind::{OnUi, Sink};
use compsdk::app::{Effect, Event, Events};
use compsdk::{Component, ImportError};
use uikit::widget::{UiError, UiEvent};

/// The component's state schema: the terminal's own.
pub const SCHEMA: u32 = state::SCHEMA;
/// The window's title.
pub const TITLE: &str = "Terminal";

/// The effect queue: `kai:app/effects`.
pub struct Effects;

impl Os for Effects {
    fn request(&mut self, name: &'static str, args: String) -> u32 {
        compsdk::app::request(Effect::Call {
            name: String::from(name),
            args,
        })
    }
}

pub struct TerminalWindow {
    pub term: Terminal,
    view: View,
    /// The open window, and the tree it was last sent.
    window: Option<(u32, Vec<Entry>)>,
}

impl TerminalWindow {
    fn new(term: Terminal) -> Self {
        Self {
            term,
            view: View::default(),
            window: None,
        }
    }

    /// The open window's id.
    pub fn window(&self) -> Option<u32> {
        self.window.as_ref().map(|(id, _)| *id)
    }

    /// Opens the window if it is closed, sends what changed since the last tree,
    /// gives the input line the focus when it has just appeared, and commits. On
    /// an error the last tree is kept, so the next call sends the changes again.
    pub fn present(&mut self, sink: &mut dyn Sink) -> Result<(), UiError> {
        let (id, mut shown) = match self.window.take() {
            Some(open) => open,
            None => {
                let (w, h) = comp_terminal_ui::SIZE;
                (sink.open(TITLE, w, h)?, Vec::new())
            }
        };
        let n = next(&mut shown, &self.term, &self.view);
        let sent = send(sink, id, &n.changes)
            .and_then(|()| n.focus.map_or(Ok(()), |node| sink.focus(id, node)))
            .and_then(|()| sink.commit(id));
        if sent.is_ok() {
            self.view.sent();
        }
        self.window = Some((id, if sent.is_ok() { n.tree } else { shown }));
        sent
    }

    /// Draws the window, or logs why it could not.
    #[cfg(target_family = "wasm")]
    fn present_logged(&mut self) {
        if let Err(e) = self.present(&mut comp_ui_bind::bind::View) {
            compsdk::log(&alloc::format!("terminal window: {e:?}"));
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn present_logged(&mut self) {}

    /// `terminal_run` and `terminal_clear` run `f` on the model, then redraw to
    /// show what it did.
    pub fn act(
        &mut self,
        f: impl FnOnce(&mut Terminal, &mut Effects) -> Result<String, String>,
    ) -> Result<String, String> {
        let said = f(&mut self.term, &mut Effects);
        self.present_logged();
        said
    }
}

/// Sends `changes` to `window`, stopping at the first the host refuses.
fn send(sink: &mut dyn Sink, window: u32, changes: &[Change]) -> Result<(), UiError> {
    changes.iter().try_for_each(|change| match change {
        Change::Put(e) => sink.put(
            window,
            &tree::entry(e.id, e.parent, e.layout, e.widget.clone()),
        ),
        Change::Remove(node) => sink.remove(window, *node),
    })
}

impl OnUi for TerminalWindow {
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError> {
        if self.window() != Some(window) {
            return Ok(());
        }
        if *event == UiEvent::CloseRequested {
            self.window = None;
            return sink.close(window);
        }
        match on_ui(&mut self.term, &mut Effects, &mut self.view, event) {
            true => self.present(sink),
            false => Ok(()),
        }
    }
}

impl Events for TerminalWindow {
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Started => {}
            Event::EffectDone { ticket, outcome } => {
                if self.term.done(&mut Effects, ticket, outcome) {
                    self.present_logged();
                }
            }
        }
    }
}

impl Component for TerminalWindow {
    const SCHEMA: u32 = SCHEMA;

    /// A terminal that has said hello, in a window.
    fn init() -> Self {
        let mut app = Self::new(Terminal::new());
        app.present_logged();
        app
    }

    /// The scrollback the last version kept.
    fn import(bytes: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        let kept: State = compsdk::state::decode(bytes, SCHEMA).ok_or(ImportError::Bad)?;
        let mut app = Self::new(Terminal::from_state(kept));
        app.present_logged();
        Ok(app)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| compsdk::state::encode(SCHEMA, &self.term.to_state()))
    }

    fn health(&self) -> u32 {
        0
    }

    fn self_test(&self) -> u32 {
        0
    }
}

compsdk::component!(TerminalWindow);
compsdk::events!(TerminalWindow);
#[cfg(target_family = "wasm")]
comp_ui_bind::input!(TerminalWindow);
