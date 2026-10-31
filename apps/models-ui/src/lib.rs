//! The model manager's window on `kai:ui` (ADR 0005 section 4; P4 task 10): a
//! table of the stored and signed models with each one's licence (ADR 0009),
//! the chosen model's details, and the attached disks a model can be imported
//! from. As Notes' window does, it rebuilds the tree whole from the model after
//! each event and [`diff`]s it against the last one. A choice comes back from
//! [`on_ui`] as the model's [`Intent`], which the component carries out through
//! the agent's rows, so the window itself changes nothing in the OS.

#![no_std]

extern crate alloc;

mod tree;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_models::{Import, Intent, Models, Row, Sha256, State, hex, size};
pub use tree::{Change, Entry, diff};
use tree::{entry, label, row, wide};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Layout, NodeId, ROOT, UiEvent, Value, Widget};

pub const COLUMN: NodeId = 1;
pub const HEADING: NodeId = 2;
pub const TABLE: NodeId = 3;
pub const DETAIL: NodeId = 4;
pub const USE: NodeId = 5;
pub const DISKS: NodeId = 6;
pub const NO_DISKS: NodeId = 7;
pub const IMPORTING: NodeId = 8;
/// The status lines: a trial, a rollback, the next restart's model.
pub const STATUS: NodeId = 10;
/// The chosen model's lines.
pub const DETAIL_LINE: NodeId = 20;
// Disks take a block of 1,000 ids each, and the window shows at most
// [`MAX_ROWS`] of them, so no two nodes can share an id.
pub const DISK: NodeId = 1000;
pub const DISK_TEXT: NodeId = 2000;
pub const IMPORT: NodeId = 3000;

/// Most models the table shows, and most disks.
pub const MAX_ROWS: usize = 256;
/// Most characters of a name or a source a cell or line shows: the role's text
/// is the role's, and the tree has a size cap (ADR 0005 section 3).
pub const MAX_CHARS: usize = 64;
/// The window's size.
pub const SIZE: (u16, u16) = (640, 460);
pub const COLUMNS: [&str; 4] = ["Model", "Size", "Licence", "State"];

/// What the window shows that the model does not hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The chosen model, by its SHA-256, so it stays chosen as the list moves.
    pub selected: Option<Sha256>,
}

/// `text`, cut to [`MAX_CHARS`] characters with an ellipsis.
pub fn clip(text: &str) -> String {
    match text.char_indices().nth(MAX_CHARS) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => String::from(text),
    }
}

/// What the list says of a model's state.
pub fn state(r: &Row<'_>, on_trial: bool) -> &'static str {
    match r.state {
        State::Running if on_trial => "in use, on trial",
        State::Running => "in use",
        State::Next => "from the next restart",
        State::Stored => "stored",
        State::Available => "signed, not stored",
    }
}

/// A model's name by its hash, as the list gives it.
fn name_of(m: &Models, sha256: &Sha256) -> String {
    m.row(sha256)
        .map_or_else(|| format!("sha256 {}", &hex(sha256)[..12]), |r| r.name())
}

/// The status lines over the table.
pub fn status(m: &Models) -> Vec<String> {
    let running = m.store.running.map(|s| name_of(m, &s));
    let mut lines = Vec::new();
    if let Some((from, why)) = &m.store.rolled_back {
        let back = running.as_deref().unwrap_or("no model");
        lines.push(format!(
            "{} did not pass its trial ({why}); back on {back}.",
            clip(&name_of(m, from))
        ));
    }
    if let (true, Some(r)) = (m.store.on_trial, &running) {
        lines.push(format!(
            "{} is on trial: it is kept once it loads and passes the smoke tasks.",
            clip(r)
        ));
    }
    if let Some(next) = m.store.next.filter(|n| Some(*n) != m.store.running) {
        lines.push(format!(
            "{} is used from the next restart.",
            clip(&name_of(m, &next))
        ));
    }
    lines
}

/// The window's tree for `models` as `view` shows it.
pub fn build(models: &Models, view: &View) -> Vec<Entry> {
    let mut tree = alloc::vec![
        entry(
            COLUMN,
            ROOT,
            tree::column(12, 8),
            Widget::Stack { axis: Axis::Column }
        ),
        entry(
            HEADING,
            COLUMN,
            wide(),
            label("Models", TypeStyle::Heading, Tone::Normal)
        ),
    ];
    for (i, text) in status(models).iter().enumerate() {
        tree.push(entry(
            STATUS + i as u32,
            COLUMN,
            wide(),
            label(text, TypeStyle::Body, Tone::Normal),
        ));
    }
    let rows: Vec<Row<'_>> = models.rows().into_iter().take(MAX_ROWS).collect();
    let selected = view
        .selected
        .and_then(|s| rows.iter().position(|r| r.sha256 == s));
    tree.push(entry(
        TABLE,
        COLUMN,
        wide(),
        Widget::Table {
            label: String::from("Models"),
            columns: COLUMNS.iter().map(|c| String::from(*c)).collect(),
            rows: rows
                .iter()
                .map(|r| {
                    alloc::vec![
                        clip(&r.name()),
                        size(r.length),
                        clip(r.licence()),
                        String::from(state(r, models.store.on_trial)),
                    ]
                })
                .collect(),
            selected: selected.map(|i| i as u32),
        },
    ));
    if let Some(r) = selected.map(|i| &rows[i]) {
        detail(models, r, &mut tree);
    }
    disks(models, &mut tree);
    tree
}

fn detail(m: &Models, r: &Row<'_>, tree: &mut Vec<Entry>) {
    tree.push(entry(
        DETAIL,
        COLUMN,
        wide(),
        Widget::Stack { axis: Axis::Column },
    ));
    let mut lines = alloc::vec![
        clip(&r.name()),
        format!("SHA-256 {}", hex(&r.sha256)),
        format!("Licence: {}", clip(r.licence())),
    ];
    if let Some(s) = r.signed {
        lines.push(format!("From {}", clip(&s.source)));
    }
    for (i, text) in lines.iter().enumerate() {
        tree.push(entry(
            DETAIL_LINE + i as u32,
            DETAIL,
            wide(),
            label(text, TypeStyle::Body, Tone::Normal),
        ));
    }
    if m.use_model(&r.sha256).is_some() {
        tree.push(entry(
            USE,
            DETAIL,
            Layout::FIT,
            Widget::Button {
                text: String::from("Use from the next restart"),
                action: Some(String::from("model_use")),
            },
        ));
    }
}

fn disks(m: &Models, tree: &mut Vec<Entry>) {
    tree.push(entry(
        DISKS,
        COLUMN,
        wide(),
        label("Attached disks", TypeStyle::Label, Tone::Normal),
    ));
    if m.disks.is_empty() {
        tree.push(entry(
            NO_DISKS,
            COLUMN,
            wide(),
            label(
                "Attach a disk that holds a model to import it.",
                TypeStyle::Body,
                Tone::Muted,
            ),
        ));
    }
    for (i, d) in m.disks.iter().take(MAX_ROWS).enumerate() {
        let at = DISK + i as u32;
        let can = m.import(d.id).is_some();
        let fits = comp_models::could_be(&m.signed, d.length);
        let (note, tone) = if fits {
            ("", Tone::Normal)
        } else {
            (", not the size of a model the role signs", Tone::Muted)
        };
        let text = format!("{}: {}{note}", clip(&d.label), size(d.length));
        tree.push(entry(at, COLUMN, wide(), row()));
        tree.push(entry(
            DISK_TEXT + i as u32,
            at,
            wide(),
            label(&text, TypeStyle::Body, tone),
        ));
        if can {
            tree.push(entry(
                IMPORT + i as u32,
                at,
                Layout::FIT,
                Widget::Button {
                    text: String::from("Import"),
                    action: None,
                },
            ));
        }
    }
    if let Some(import) = &m.import {
        tree.push(importing(m, import));
    }
}

/// The running import's progress, or the last one's result.
fn importing(m: &Models, import: &Import) -> Entry {
    let disk = |id: u32| {
        m.disks
            .iter()
            .find(|d| d.id == id)
            .map_or_else(|| format!("disk {id}"), |d| clip(&d.label))
    };
    let widget = match import {
        Import::Copying {
            disk: id,
            done,
            length,
        } => Widget::Progress {
            label: format!("Importing from {}, checking its SHA-256", disk(*id)),
            done: (*length > 0)
                .then(|| (u128::from(*done) * 1000 / u128::from(*length)).min(1000) as u16),
        },
        Import::Stored { disk: id, sha256 } => label(
            &format!(
                "Imported {} from {}: the signed models role names it.",
                clip(&name_of(m, sha256)),
                disk(*id)
            ),
            TypeStyle::Body,
            Tone::Normal,
        ),
        Import::Refused { disk: id, why } => label(
            &format!("{} was not imported: {}.", disk(*id), why.why()),
            TypeStyle::Body,
            Tone::Normal,
        ),
    };
    entry(IMPORTING, COLUMN, wide(), widget)
}

/// `id` as the `i`th of the rows starting at `base`, if it is one.
fn nth(id: NodeId, base: NodeId) -> Option<usize> {
    id.checked_sub(base)
        .map(|i| i as usize)
        .filter(|&i| i < MAX_ROWS)
}

/// Handles one event: changes `view` for a selection, and answers a choice
/// with the intent to carry out. True if the window must be drawn again.
pub fn on_ui(models: &Models, view: &mut View, e: &UiEvent) -> (bool, Option<Intent>) {
    match e {
        UiEvent::Changed(TABLE, Value::Index(i)) => {
            let rows = models.rows();
            let chosen = rows.get(*i as usize).filter(|_| (*i as usize) < MAX_ROWS);
            view.selected = chosen.map(|r| r.sha256);
            (true, None)
        }
        UiEvent::Pressed(USE) => (false, view.selected.and_then(|s| models.use_model(&s))),
        UiEvent::Pressed(id) => {
            let disk = nth(*id, IMPORT).and_then(|i| models.disks.get(i));
            (false, disk.and_then(|d| models.import(d.id)))
        }
        _ => (false, None),
    }
}
