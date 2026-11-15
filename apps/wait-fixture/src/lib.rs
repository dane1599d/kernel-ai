//! A test fixture for the `wait-reads` boot case (the founder's test 2, item
//! 7), never shipped. [`Fixture`] is a window with two buttons, as the app
//! `wait-window`: a key on Read queues one `fs_list` of the home folder, a key
//! on Make one `fs_create` of a new file, each as a person's effect, through
//! `desktop` to the shell. When the OS says what came of one, the app logs it
//! with how long it took from the key, by its clock, and keeps one line for
//! it. Its one action, `seen`, gives the case how many keys it took and those
//! lines in the order the answers came, so the case can tell what was answered
//! while the model thought and what waited for the line.
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

#[cfg(target_family = "wasm")]
mod bind;

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// The window's title.
pub const TITLE: &str = "Wait";
/// The window's size.
pub const SIZE: (u16, u16) = (320, 120);
/// The buttons: Tab goes from one to the other, round.
pub const READ: NodeId = 3;
pub const MAKE: NodeId = 4;
/// The most presses it keeps track of.
pub const MAX_PRESSES: u32 = 32;

/// What a press was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Read,
    Make,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Make => "make",
        }
    }
}

/// The window's tree.
pub fn build() -> Vec<Entry> {
    let button = |id, text: &str| {
        tree::entry(
            id,
            1,
            Layout::FIT,
            Widget::Button {
                text: String::from(text),
                action: None,
            },
        )
    };
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
                "Test fixture: reads and changes while Kai thinks.",
                TypeStyle::Body,
                Tone::Normal
            ),
        ),
        button(READ, "Read"),
        button(MAKE, "Make"),
    ]
}

/// Logs a line, on the target only.
fn log(text: &str) {
    #[cfg(target_family = "wasm")]
    compsdk::log(text);
    #[cfg(not(target_family = "wasm"))]
    let _ = text;
}

pub struct Fixture {
    shown: Shown,
    /// Presses so far.
    presses: u32,
    /// Effects queued and not answered yet: ticket, kind, the clock at the key.
    waiting: Vec<(u32, Kind, u64)>,
    /// One line for each answer, in the order they came.
    answered: Vec<String>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            shown: Shown::new(),
            presses: 0,
            waiting: Vec::new(),
            answered: Vec::new(),
        }
    }

    /// Queues the effect a press on `kind`'s button asks for, at `now_ms`.
    pub fn press(&mut self, kind: Kind, now_ms: u64) {
        if self.presses >= MAX_PRESSES {
            return;
        }
        self.presses += 1;
        let (name, args) = match kind {
            Kind::Read => ("fs_list", String::from("{path: \"\"}")),
            Kind::Make => (
                "fs_create",
                format!(
                    "{{path: \"made-{}.txt\", text: \"made by wait-window\"}}",
                    self.presses
                ),
            ),
        };
        let ticket = compsdk::app::request(Effect::Call {
            name: String::from(name),
            args,
        });
        self.waiting.push((ticket, kind, now_ms));
    }

    /// What came of `ticket`, at `now_ms`; an answer to no effect it is
    /// waiting on (a notice from the OS) is not kept.
    pub fn answer(&mut self, ticket: u32, ok: bool, now_ms: u64) {
        let Some(at) = self.waiting.iter().position(|(t, ..)| *t == ticket) else {
            return;
        };
        let (_, kind, pressed) = self.waiting.remove(at);
        let word = if ok { "ok" } else { "err" };
        let took = now_ms.saturating_sub(pressed);
        log(&format!("wait-window: {} {word} in {took} ms", kind.word()));
        self.answered
            .push(format!("{ticket} {} {word} {took}", kind.word()));
    }

    /// `pressed <n>`, then one line for each answer so far, in the order they
    /// came: `<ticket> <kind> ok|err <ms from the key>`.
    pub fn seen(&self) -> String {
        let mut lines = alloc::vec![format!("pressed {}", self.presses)];
        lines.extend(self.answered.iter().cloned());
        lines.join("\n")
    }

    /// Draws the window through `sink`: opens it if it is closed.
    pub fn present(&mut self, sink: &mut dyn Sink) -> Result<(), UiError> {
        self.shown.present(sink, TITLE, SIZE, build())
    }

    #[cfg(target_family = "wasm")]
    fn present_logged(&mut self) {
        if let Err(e) = self.present(&mut comp_ui_bind::bind::View) {
            compsdk::log(&format!("wait-window: {e:?}"));
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn present_logged(&mut self) {}
}

impl OnUi for Fixture {
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError> {
        if self.shown.id() != Some(window) {
            return Ok(());
        }
        match event {
            UiEvent::CloseRequested => self.shown.closed(sink, window).map(|_| ()),
            UiEvent::Pressed(READ) => {
                self.press(Kind::Read, compsdk::app::now_ms());
                Ok(())
            }
            UiEvent::Pressed(MAKE) => {
                self.press(Kind::Make, compsdk::app::now_ms());
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl Events for Fixture {
    fn on_event(&mut self, event: Event) {
        if let Event::EffectDone { ticket, outcome } = event {
            self.answer(ticket, outcome.is_ok(), compsdk::app::now_ms());
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

    /// The fixture keeps nothing, but the host asks every version for a state
    /// it can read back, so the state is an empty one.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        compsdk::state::decode::<()>(state, SCHEMA).ok_or(ImportError::Bad)?;
        Ok(Self::init())
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
    fn answers_are_kept_in_the_order_they_come_once_with_their_time() {
        let mut app = Fixture::new();
        app.waiting = alloc::vec![
            (1, Kind::Read, 100),
            (2, Kind::Make, 200),
            (3, Kind::Read, 300)
        ];
        app.presses = 3;
        app.answer(2, true, 950);
        app.answer(1, false, 1_000);
        // A second answer, or one for a ticket it never sent, is not kept.
        app.answer(2, true, 2_000);
        app.answer(9, true, 2_000);
        app.answer(3, true, 400);
        assert_eq!(
            app.seen(),
            "pressed 3\n2 make ok 750\n1 read err 900\n3 read ok 100"
        );
        assert!(app.waiting.is_empty());
    }

    #[test]
    fn the_window_is_a_label_and_two_buttons_tab_goes_between() {
        use uikit::input::{Input, Key, Mods};
        use uikit::layout::FixedAdvance;
        use uikit::window::Window;

        let mut window = Window::new(TITLE, SIZE.0, SIZE.1);
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
        // The keys the `wait-reads` case sends: tab ret reads, then tab ret
        // makes, and tab comes round to Read again.
        key(Key::Tab);
        assert!(key(Key::Enter).contains(&UiEvent::Pressed(READ)));
        key(Key::Tab);
        assert!(key(Key::Enter).contains(&UiEvent::Pressed(MAKE)));
        key(Key::Tab);
        assert!(key(Key::Enter).contains(&UiEvent::Pressed(READ)));
    }
}
