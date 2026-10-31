//! The System monitor in the OS (P4 task 9): [`MonitorWindow`] is `comp-monitor`'s
//! model with the window `comp-monitor-ui` builds from it. It opens its window
//! when it starts and asks the host for a sample of the machine (`kai:system`,
//! granted to this app alone); each sample that comes back updates the model, is
//! drawn, and is followed by the next read, which the host paces to one a second.
//! It queues no effect: stopping a process is the OS's `process_stop`, which asks,
//! and the agent core runs only the file rows for an app, so the window tells the
//! user to ask Kai.
//!
//! When the window is closed the app stops reading, so nothing waits on the host
//! and the app can nap like any other. A sample or an answer that arrives after
//! that draws nothing.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;

use alloc::string::String;
use alloc::vec::Vec;
use comp_monitor::sample::Sample;
use comp_monitor::{Monitor, Sort};
use comp_ui_bind::{OnUi, Shown, Sink};
use compsdk::app::{Event, Events};
use compsdk::{Component, ImportError};
use minicbor::{Decode, Encode};
use uikit::widget::{UiError, UiEvent};

/// The component's state schema.
pub const SCHEMA: u32 = 1;

/// What the checkpoint keeps: how the table was sorted, so a restart comes back
/// to it. The numbers are read again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V1 {
    #[n(0)]
    pub sort: String,
    #[n(1)]
    pub flipped: bool,
}

pub struct MonitorWindow {
    pub monitor: Monitor,
    shown: Shown,
    /// Whether the window is still wanted: it is not once the user closed it.
    open: bool,
    /// Whether a read is out, asked for and not yet answered.
    reading: bool,
}

impl MonitorWindow {
    fn new(monitor: Monitor) -> Self {
        Self {
            monitor,
            shown: Shown::new(),
            open: true,
            reading: false,
        }
    }

    /// Draws the window through `sink`: opens it if it is closed.
    pub fn present(&mut self, sink: &mut dyn Sink) -> Result<(), UiError> {
        let tree = comp_monitor_ui::build(&self.monitor);
        self.shown
            .present(sink, comp_monitor_ui::TITLE, comp_monitor_ui::SIZE, tree)
    }

    /// Draws the window, or logs why it could not; nothing once it was closed.
    #[cfg(target_family = "wasm")]
    fn present_logged(&mut self) {
        if !self.open {
            return;
        }
        if let Err(e) = self.present(&mut comp_ui_bind::bind::View) {
            compsdk::log(&alloc::format!("monitor window: {e:?}"));
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn present_logged(&mut self) {}

    /// The window's open id, if it is open.
    pub fn window(&self) -> Option<u32> {
        self.shown.id()
    }

    /// Whether a read of the machine is out.
    pub fn reading(&self) -> bool {
        self.reading
    }

    /// Asks the host for the next sample, if the window is wanted and no read is
    /// out already.
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

    /// A sample came back for a read, or the reason there is none. What is not a
    /// sample is a reading the window does not show, and it says so.
    pub fn sampled(&mut self, outcome: Result<Vec<u8>, String>) {
        self.reading = false;
        match outcome.map(|bytes| Sample::parse(&bytes)) {
            Ok(Ok(sample)) => self.monitor.update(&sample),
            Ok(Err(_)) => self
                .monitor
                .no_reading("the sample came in a form this version does not know"),
            Err(why) => self.monitor.no_reading(&why),
        }
        self.present_logged();
        self.ask();
    }

    /// `monitor_sort` and `monitor_select` run `f` on the model, then redraw to
    /// show it.
    pub fn act(
        &mut self,
        f: impl FnOnce(&mut Monitor) -> Result<String, String>,
    ) -> Result<String, String> {
        let said = f(&mut self.monitor);
        self.present_logged();
        said
    }
}

impl OnUi for MonitorWindow {
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError> {
        if self.shown.id() != Some(window) {
            return Ok(());
        }
        if *event == UiEvent::CloseRequested {
            self.open = false;
            return self.shown.closed(sink, window).map(|_| ());
        }
        match comp_monitor_ui::on_ui(&mut self.monitor, event) {
            true => self.present(sink),
            false => Ok(()),
        }
    }
}

impl Events for MonitorWindow {
    /// Nothing queues an effect, so there is no answer to wait for; `started` needs
    /// nothing either (the first read was asked for in `init`).
    fn on_event(&mut self, _event: Event) {}
}

impl Component for MonitorWindow {
    const SCHEMA: u32 = SCHEMA;

    /// A window with nothing in it, and the first read; the numbers come with
    /// the host's answer.
    fn init() -> Self {
        let mut app = Self::new(Monitor::new());
        app.present_logged();
        app.ask();
        app
    }

    /// The sort the last version showed; anything else starts as new.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        let v1: V1 = compsdk::state::decode(state, SCHEMA).ok_or(ImportError::Bad)?;
        let mut monitor = Monitor::new();
        if let Some(sort) = Sort::parse(&v1.sort) {
            monitor.set_sort(sort);
            if v1.flipped {
                monitor.press_sort(sort);
            }
        }
        let mut app = Self::new(monitor);
        app.present_logged();
        app.ask();
        Ok(app)
    }

    fn export(&self, schema: u32) -> Option<Vec<u8>> {
        (schema == SCHEMA).then(|| {
            let v1 = V1 {
                sort: String::from(self.monitor.sort().word()),
                flipped: self.monitor.flipped(),
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

compsdk::component!(MonitorWindow);
compsdk::events!(MonitorWindow);
#[cfg(target_family = "wasm")]
comp_ui_bind::input!(MonitorWindow);
