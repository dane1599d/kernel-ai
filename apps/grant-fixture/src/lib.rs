//! A test fixture for the `app-settings` boot case (P4 task 7), never shipped.
//! [`Fixture`] is a window with one button. Pressing it (a key in the window:
//! Tab to the button, Enter) queues one effect, the read-only `fs_list`, and
//! when the OS says what came of it the app logs `fixture: fs_list ok` or
//! `fixture: fs_list refused: <why>`. The case revokes the row in Settings,
//! presses the button and sees the refusal, restores the row and sees the OS
//! run it again: the end-to-end proof that Settings' revoke holds.
//!
//! Only test images carry it (`xtask::components::TEST_ONLY`).

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_ui_bind::tree::{self, Entry};
use comp_ui_bind::{OnUi, Shown, Sink};
use compsdk::app::{Effect, Event, Events};
use compsdk::{Component, ImportError};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Layout, NodeId, ROOT, UiError, UiEvent, Widget};

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// The window's title.
pub const TITLE: &str = "Fixture";
/// The window's size.
pub const SIZE: (u16, u16) = (320, 120);
/// The button.
pub const PING: NodeId = 3;
/// The one row the app queues, and its arguments: the home folder.
pub const ROW: &str = "fs_list";
const ARGS: &str = "{path: \"\"}";
/// The longest reason logged.
const MAX_WHY: usize = 120;

/// The window's tree.
pub fn build() -> Vec<Entry> {
    alloc::vec![
        tree::entry(
            1,
            ROOT,
            tree::column(8, 8),
            Widget::Stack { axis: Axis::Column }
        ),
        tree::entry(
            2,
            1,
            tree::wide(),
            tree::label(
                "Test fixture: queues one fs_list.",
                TypeStyle::Body,
                Tone::Normal
            ),
        ),
        tree::entry(
            PING,
            1,
            Layout::FIT,
            Widget::Button {
                text: String::from("Ping"),
                action: None,
            },
        ),
    ]
}

/// Logs a line, on the target only.
fn log(text: &str) {
    #[cfg(target_family = "wasm")]
    compsdk::log(text);
    #[cfg(not(target_family = "wasm"))]
    let _ = text;
}

/// What the line for the `press`th press's outcome says: its words cut to a
/// line. The number tells the presses apart in a log.
pub fn outcome_line(press: u32, outcome: &Result<String, String>) -> String {
    match outcome {
        Ok(_) => format!("fixture: {ROW} ok #{press}"),
        Err(why) => {
            let why: String = why
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(MAX_WHY)
                .collect();
            format!("fixture: {ROW} refused #{press}: {why}")
        }
    }
}

pub struct Fixture {
    shown: Shown,
    /// The ticket of the effect waiting for its answer, and its press.
    waiting: Option<(u32, u32)>,
    /// Presses so far.
    presses: u32,
}

impl Fixture {
    fn new() -> Self {
        Self {
            shown: Shown::new(),
            waiting: None,
            presses: 0,
        }
    }

    /// Draws the window through `sink`: opens it if it is closed.
    pub fn present(&mut self, sink: &mut dyn Sink) -> Result<(), UiError> {
        self.shown.present(sink, TITLE, SIZE, build())
    }

    #[cfg(target_family = "wasm")]
    fn present_logged(&mut self) {
        if let Err(e) = self.present(&mut comp_ui_bind::bind::View) {
            compsdk::log(&format!("fixture window: {e:?}"));
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn present_logged(&mut self) {}

    /// The window's open id, if it is open.
    pub fn window(&self) -> Option<u32> {
        self.shown.id()
    }
}

impl OnUi for Fixture {
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError> {
        if self.shown.id() != Some(window) {
            return Ok(());
        }
        match event {
            UiEvent::CloseRequested => self.shown.closed(sink, window).map(|_| ()),
            UiEvent::Pressed(PING) => {
                let ticket = compsdk::app::request(Effect::Call {
                    name: String::from(ROW),
                    args: String::from(ARGS),
                });
                self.presses += 1;
                self.waiting = Some((ticket, self.presses));
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl Events for Fixture {
    fn on_event(&mut self, event: Event) {
        if let Event::EffectDone { ticket, outcome } = event
            && let Some((waiting, press)) = self.waiting
            && waiting == ticket
        {
            self.waiting = None;
            log(&outcome_line(press, &outcome));
        }
    }
}

impl Component for Fixture {
    const SCHEMA: u32 = SCHEMA;

    fn init() -> Self {
        let mut app = Self::new();
        app.present_logged();
        app
    }

    /// The fixture keeps nothing, but the host asks every version for a state it
    /// can read back, so the state is an empty one and a moved fixture starts
    /// again.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        compsdk::state::decode::<()>(state, SCHEMA).ok_or(ImportError::Bad)?;
        let mut app = Self::new();
        app.present_logged();
        Ok(app)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| compsdk::state::encode(SCHEMA, &()))
    }

    fn health(&self) -> u32 {
        0
    }

    fn self_test(&self) -> u32 {
        0
    }
}

compsdk::component!(Fixture);
compsdk::events!(Fixture);
#[cfg(target_family = "wasm")]
comp_ui_bind::input!(Fixture);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_outcome_line_says_ok_or_the_refusal_cut_to_a_line() {
        assert_eq!(
            outcome_line(1, &Ok(String::from("a b"))),
            "fixture: fs_list ok #1"
        );
        assert_eq!(
            outcome_line(2, &Err(String::from("withheld by the user\nnext"))),
            "fixture: fs_list refused #2: withheld by the user next"
        );
        let long = outcome_line(3, &Err("x".repeat(500)));
        assert_eq!(long.len(), "fixture: fs_list refused #3: ".len() + MAX_WHY);
    }

    #[test]
    fn the_window_is_a_label_and_one_button() {
        let tree = build();
        assert_eq!(tree.len(), 3);
        assert!(tree.iter().any(|e| e.id == PING));
    }

    /// The keys the `app-settings` case sends (`tab`, `ret`) press the button in a
    /// real `uikit::Window`, the way a person at the keyboard would.
    #[test]
    fn tab_then_enter_presses_the_button() {
        use uikit::input::{Input, Key, Mods};
        use uikit::layout::FixedAdvance;
        use uikit::window::Window;

        let mut window = Window::new("Fixture", SIZE.0, SIZE.1);
        for e in build() {
            window.put(e.id, e.parent, e.layout, e.widget).unwrap();
        }
        window.commit(&FixedAdvance);
        let mut key = |key| {
            let ev = Input::Key {
                key,
                down: true,
                mods: Mods::default(),
                text: String::new(),
            };
            window.input(ev, &FixedAdvance)
        };
        assert!(!key(Key::Tab).contains(&UiEvent::Pressed(PING)));
        assert!(key(Key::Enter).contains(&UiEvent::Pressed(PING)));
    }
}
