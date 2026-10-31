//! The Settings app in the OS (P4 task 7): [`SettingsWindow`] is
//! `comp-settings`'s model with the window `comp-settings-ui` builds from it. It
//! opens its window when it starts and redraws after each event. It reads and
//! changes the system through `kai:settings`, a granted interface
//! (`wit/kai-settings/settings.wit`): when it starts it asks for everything the
//! pages show ([`replies::Read`]); a choice in the window becomes a request
//! ([`requests`]); and the pages change only when the answer says it is done.
//! A refused request, such as a change comphost will not pass on because no
//! person pressed the key, is put back by the redraw.
//!
//! The requests are queued by `send` and answered in a later call
//! (`replies.on-reply`), so nothing here waits on the system.

#![cfg_attr(target_family = "wasm", no_std)]

extern crate alloc;

#[cfg(target_family = "wasm")]
mod bind;
pub mod replies;
pub mod requests;

use alloc::string::String;
use alloc::vec::Vec;
use comp_settings::{Intent, Page, Settings};
use comp_settings_ui::{Change, Entry, SIZE, View};
use comp_ui_bind::tree;
use comp_ui_bind::{OnUi, Sink};
use compsdk::app::{Event, Events};
use compsdk::{Component, ImportError};
use minicbor::{Decode, Encode};
use replies::Read;
use requests::Applied;
use uikit::widget::{UiError, UiEvent};

/// The component's state schema.
pub const SCHEMA: u32 = 1;
/// The window's title.
pub const TITLE: &str = "Settings";
/// Most requests waiting for an answer.
const MAX_PENDING: usize = 32;
/// Most `grants <app>` reads waiting at once. The host keeps at most
/// `wasmhost::settings::MAX_QUEUED` requests of a call, and the shell lists up
/// to 32 apps, so the rest are asked as these are answered.
const ROWS_IN_FLIGHT: usize = 4;

/// What the checkpoint keeps: nothing. The page shown is not state, because
/// `settings_open` moves it and is a read-only row, and the host refuses a
/// read-only row that changes the checkpoint (the desktop tour caught it). A
/// restart starts on the first page; everything the pages show is read again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct V1 {}

/// The window the app has open, and the tree it was last sent.
#[derive(Default)]
struct Shown {
    window: Option<(u32, Vec<Entry>)>,
}

/// `comp-settings-ui`'s entry as `kai:ui/view` takes it. The two crates keep
/// separate copies of the node type until `uikit` holds one.
fn entry(e: &Entry) -> tree::Entry {
    tree::Entry {
        id: e.id,
        parent: e.parent,
        layout: e.layout,
        widget: e.widget.clone(),
    }
}

/// Opens the window and keeps it from the agent before any of its tree is
/// sent: the rules and grants it shows are typed-only data, and Kai must not
/// read them or park focus on Revoke. If the system will not keep it private
/// the window is closed again, so it is never shown shared.
fn open_private(sink: &mut dyn Sink) -> Result<u32, UiError> {
    let id = sink.open(TITLE, SIZE.0, SIZE.1)?;
    if let Err(e) = sink.set_agent_private(id, true) {
        let _ = sink.close(id);
        return Err(e);
    }
    Ok(id)
}

impl Shown {
    /// Opens the window if it is closed, sends what changed to reach `next`
    /// ([`comp_settings_ui::redraw`], which puts back a control the model
    /// refused) and commits. On an error the last tree is kept, so the next
    /// call sends the changes again.
    fn present(
        &mut self,
        sink: &mut dyn Sink,
        next: Vec<Entry>,
        view: &mut View,
    ) -> Result<(), UiError> {
        let (id, last) = match self.window.take() {
            Some(open) => open,
            None => (open_private(sink)?, Vec::new()),
        };
        let sent = comp_settings_ui::redraw(&last, &next, view)
            .iter()
            .try_for_each(|c| match c {
                Change::Put(e) => sink.put(id, &entry(e)),
                Change::Remove(node) => sink.remove(id, *node),
            })
            .and_then(|()| sink.commit(id));
        self.window = Some((id, if sent.is_ok() { next } else { last }));
        sent
    }

    fn id(&self) -> Option<u32> {
        self.window.as_ref().map(|(id, _)| *id)
    }
}

/// What a request is waiting to do when its answer comes.
enum Pending {
    /// Put the answer on a page.
    Read(Read),
    /// Put the answer on a page as the rows of this app.
    Rows(String),
    /// Show the change.
    Change(Applied),
}

/// `kai:settings/settings.send`; on the host, where there is no system, the
/// ticket of a request that is never answered.
fn send(request: &str) -> u32 {
    #[cfg(target_family = "wasm")]
    return bind::send(request);
    #[cfg(not(target_family = "wasm"))]
    {
        let _ = request;
        0
    }
}

fn log(text: &str) {
    #[cfg(target_family = "wasm")]
    compsdk::log(text);
    #[cfg(not(target_family = "wasm"))]
    let _ = text;
}

/// The words `settings_open` takes, in the order of [`Page::ALL`].
const PAGE_WORDS: [&str; 6] = ["appearance", "access", "rules", "apps", "updates", "about"];

pub struct SettingsWindow {
    pub settings: Settings,
    view: View,
    shown: Shown,
    /// Requests sent and not answered, by ticket.
    pending: Vec<(u32, Pending)>,
    /// Apps whose rows are still to be asked for, next first.
    to_ask: Vec<String>,
}

impl SettingsWindow {
    fn new(page: usize) -> Self {
        Self {
            settings: Settings::default(),
            view: View::page(page),
            shown: Shown::default(),
            pending: Vec::new(),
            to_ask: Vec::new(),
        }
    }

    /// Draws the window through `sink`: opens it if it is closed.
    pub fn present(&mut self, sink: &mut dyn Sink) -> Result<(), UiError> {
        let tree = comp_settings_ui::build(&self.settings, &mut self.view);
        self.shown.present(sink, tree, &mut self.view)
    }

    /// Draws the window, or logs why it could not.
    #[cfg(target_family = "wasm")]
    fn present_logged(&mut self) {
        if let Err(e) = self.present(&mut comp_ui_bind::bind::View) {
            compsdk::log(&alloc::format!("settings window: {e:?}"));
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn present_logged(&mut self) {}

    /// The window's open id, if it is open.
    pub fn window(&self) -> Option<u32> {
        self.shown.id()
    }

    /// The page the window shows.
    pub fn page(&self) -> usize {
        self.view.page
    }

    fn wait_for(&mut self, ticket: u32, what: Pending) {
        if self.pending.len() == MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push((ticket, what));
    }

    /// Asks for the rows of the next apps, as many as may wait at once.
    fn ask_rows(&mut self) {
        let waiting = |p: &Vec<(u32, Pending)>| {
            p.iter()
                .filter(|(_, what)| matches!(what, Pending::Rows(_)))
                .count()
        };
        while waiting(&self.pending) < ROWS_IN_FLIGHT && !self.to_ask.is_empty() {
            let app = self.to_ask.remove(0);
            let ticket = send(&alloc::format!("grants {app}"));
            self.wait_for(ticket, Pending::Rows(app));
        }
    }

    /// Asks for everything the pages show.
    fn read_all(&mut self) {
        self.to_ask.clear();
        for read in Read::ALL {
            let ticket = send(read.request());
            self.wait_for(ticket, Pending::Read(read));
        }
    }

    /// Sends the requests for `intent`.
    fn carry_out(&mut self, intent: &Intent) {
        for p in requests::plan(&self.settings.appearance, intent) {
            let ticket = send(&p.request);
            self.wait_for(ticket, Pending::Change(p.then));
        }
    }

    /// The answer to the request with `ticket`.
    pub fn on_reply(&mut self, ticket: u32, outcome: Result<Vec<u8>, String>) {
        let Some(at) = self.pending.iter().position(|(t, _)| *t == ticket) else {
            return;
        };
        let (_, what) = self.pending.remove(at);
        match (what, outcome) {
            (_, Err(why)) => log(&alloc::format!("settings: refused: {why}")),
            (Pending::Read(read), Ok(answer)) => match read.apply(&mut self.settings, &answer) {
                Ok(apps) => {
                    // What each page was given, for the log: the window is
                    // private to the agent, so a case reads it here.
                    log(&alloc::format!(
                        "settings: {}",
                        read.summary(&self.settings, apps.len())
                    ));
                    // Only `grants` names apps to ask for; another read's answer
                    // must not empty the queue.
                    if read == Read::Grants {
                        self.to_ask = apps;
                    }
                }
                Err(why) => log(&alloc::format!("settings: {}: {why}", read.request())),
            },
            (Pending::Rows(app), Ok(answer)) => {
                if let Err(why) = replies::apply_rows(&mut self.settings, &app, &answer) {
                    log(&alloc::format!("settings: grants {app}: {why}"));
                }
            }
            (Pending::Change(then), Ok(_)) => match then {
                Applied::Theme(theme) => self.settings.appearance.theme = theme,
                Applied::Legible(on) => self.settings.appearance.legible = on,
                Applied::ReducedMotion(on) => self.settings.appearance.reduced_motion = on,
                Applied::Intent(intent) => self.settings.applied(&intent),
            },
        }
        // Whatever the answer was, the next apps' rows are asked for as room
        // comes.
        self.ask_rows();
        self.present_logged();
    }

    /// `settings_open`: shows a page, by the word the agent uses for it.
    pub fn act_open(&mut self, word: &str) -> Result<String, String> {
        let word = word.trim().to_ascii_lowercase();
        let at = PAGE_WORDS.iter().position(|w| *w == word).ok_or_else(|| {
            String::from("Pages: appearance, access, rules, apps, updates, about.")
        })?;
        self.view = View::page(at);
        self.present_logged();
        Ok(alloc::format!("Showing {}.", Page::ALL[at].title()))
    }
}

impl OnUi for SettingsWindow {
    fn on_ui(&mut self, sink: &mut dyn Sink, window: u32, event: &UiEvent) -> Result<(), UiError> {
        if self.shown.id() != Some(window) {
            return Ok(());
        }
        if *event == UiEvent::CloseRequested {
            self.shown.window = None;
            return sink.close(window);
        }
        let (redraw, intent) = comp_settings_ui::on_ui(&self.settings, &mut self.view, event);
        if let Some(intent) = intent {
            self.carry_out(&intent);
        }
        match redraw {
            true => self.present(sink),
            false => Ok(()),
        }
    }
}

impl Events for SettingsWindow {
    /// Reads the settings when the OS says the app started; it has no effects.
    fn on_event(&mut self, event: Event) {
        if event == Event::Started {
            self.read_all();
        }
    }
}

impl Component for SettingsWindow {
    const SCHEMA: u32 = SCHEMA;

    /// A window on the first page, with the model's defaults until the reads
    /// come back.
    fn init() -> Self {
        let mut app = Self::new(0);
        app.present_logged();
        app
    }

    /// A new version starts on the first page; a state it cannot read, or of
    /// another schema, is refused.
    fn import(state: &[u8], from_schema: u32) -> Result<Self, ImportError> {
        if from_schema != SCHEMA {
            return Err(ImportError::Unsupported);
        }
        let _: V1 = compsdk::state::decode(state, SCHEMA).ok_or(ImportError::Bad)?;
        let mut app = Self::new(0);
        app.present_logged();
        Ok(app)
    }

    /// The same bytes whatever page is shown.
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

compsdk::component!(SettingsWindow);
compsdk::events!(SettingsWindow);
#[cfg(target_family = "wasm")]
comp_ui_bind::input!(SettingsWindow);
