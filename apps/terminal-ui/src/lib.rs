//! The Terminal app's window on `kai:ui` (ADR 0005 section 4): the scrollback as
//! one row per line, drawn by its kind, and under it the input line with the
//! shell's prompt as its label. The tree is rebuilt whole from the model after
//! each event and [`diff`]ed against the last one, so only what changed goes to
//! the host. "Clear" is bound to `terminal_clear`, so the agent sees the same
//! words and tier as the button.
//!
//! The input is not a child of the scrollback's column, so a new row never
//! removes and puts it again, which would lose its focus and caret.

#![no_std]

extern crate alloc;

mod tree;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_terminal::span::{Kind, Span, wrap};
use comp_terminal::{MAX_LINE, Phase, Terminal};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Layout, NodeId, ROOT, Size, TextField, UiEvent, Value, Widget};

pub use tree::{Change, Entry, diff};
use tree::{column, entry, label, padded, row, wide};

pub const COLUMN: NodeId = 1;
pub const SCROLL: NodeId = 2;
pub const CONTENT: NodeId = 3;
pub const SPANS: NodeId = 4;
pub const INPUT_BOX: NodeId = 5;
/// The input line, while a line may be typed.
pub const INPUT: NodeId = 6;
/// What stands in its place while the shell works or a question waits.
pub const WAITING: NodeId = 7;
pub const FOOTER: NodeId = 8;
pub const STATUS: NodeId = 9;
pub const CLEAR: NodeId = 10;
pub const CANCEL: NodeId = 11;
/// A row of scrollback is four ids from `SPAN_BASE + (seq % SLOTS) * 4`: the row,
/// and for an input its prompt, its text and its "by the AI" tag.
pub const SPAN_BASE: NodeId = 1000;
const SLOTS: u32 = 16_000;
/// The window's size when it opens, in logical px.
pub const SIZE: (u16, u16) = (800, 520);
/// Characters a line gets before it wraps: the window's width in JetBrains Mono
/// 14 px, less its padding and scroll bar.
pub const COLS: usize = 88;
/// Rows the tree holds; older ones stay in the model and `terminal_read`.
pub const SHOWN_SPANS: usize = 100;
/// The action the Clear button runs.
pub const CLEAR_ACTION: &str = "terminal_clear";
const AI_TAG: &str = "by the AI";
const PLACEHOLDER: &str = "Type a command, or ask the AI";

/// What the window shows that the model does not hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The text the input line should hold.
    pub draft: String,
    /// The text the window's field holds now: what its last key left, or what
    /// the last tree put there. It differs from `draft` just after a line is
    /// sent, until the cleared draft reaches the window.
    pub held: String,
}

/// What an event asks of the component.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the tree may have changed.
    pub redraw: bool,
    /// A line for the shell, which the component sends as an effect.
    pub send: Option<String>,
}

fn tone(kind: Kind) -> Tone {
    match kind {
        Kind::Input | Kind::Reply => Tone::Normal,
        Kind::Error => Tone::Error,
        Kind::Question => Tone::Accent,
        Kind::Notice => Tone::Dim,
    }
}

/// `text` wrapped to `width`, each line after the first indented by `indent`.
fn wrapped(text: &str, width: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let rows = wrap(text, width.saturating_sub(indent).max(1));
    rows.join(&format!("\n{pad}"))
}

fn span_entries(span: &Span, out: &mut Vec<Entry>) {
    let base = SPAN_BASE + (span.seq % SLOTS) * 4;
    if span.kind == Kind::Input {
        let width = COLS.saturating_sub(span.prompt.chars().count()).max(20);
        out.push(entry(base, SPANS, wide(), row()));
        out.push(entry(
            base + 1,
            base,
            Layout::FIT,
            label(&span.prompt, TypeStyle::Mono, Tone::Accent),
        ));
        out.push(entry(
            base + 2,
            base,
            wide(),
            label(
                &wrapped(&span.text, width, 0),
                TypeStyle::Mono,
                Tone::Normal,
            ),
        ));
        if span.by_ai {
            out.push(entry(
                base + 3,
                base,
                Layout::FIT,
                label(AI_TAG, TypeStyle::Small, Tone::Dim),
            ));
        }
        return;
    }
    let lead = span.kind.lead();
    let text = format!(
        "{lead}{}",
        wrapped(&span.text, COLS - lead.len(), lead.len())
    );
    out.push(entry(
        base,
        SPANS,
        wide(),
        label(&text, TypeStyle::Mono, tone(span.kind)),
    ));
}

fn status(term: &Terminal) -> &'static str {
    match term.phase() {
        Phase::Ready => "Ready",
        Phase::Continuing => "The entry is open: finish it or cancel",
        Phase::Waiting => "Running",
        Phase::Asking(_) => "A question is waiting on the AI panel",
    }
}

/// The whole tree for this model and view, parents before children.
pub fn build(term: &Terminal, view: &View) -> Vec<Entry> {
    let spans = term.spans();
    let shown = &spans[spans.len().saturating_sub(SHOWN_SPANS)..];
    let mut tree = alloc::vec![
        entry(COLUMN, ROOT, Layout::FILL, column()),
        entry(
            SCROLL,
            COLUMN,
            Layout::FILL,
            Widget::Scroll { axis: Axis::Column }
        ),
        entry(CONTENT, SCROLL, padded(10, 6), column()),
        entry(SPANS, CONTENT, wide(), column()),
    ];
    for span in shown {
        span_entries(span, &mut tree);
    }
    tree.push(entry(INPUT_BOX, CONTENT, wide(), column()));
    tree.push(if term.takes_input() {
        entry(
            INPUT,
            INPUT_BOX,
            wide(),
            Widget::TextLine(TextField {
                label: String::from(term.prompt().trim_end()),
                value: view.draft.clone(),
                placeholder: String::from(PLACEHOLDER),
                max_len: MAX_LINE as u32,
            }),
        )
    } else {
        entry(
            WAITING,
            INPUT_BOX,
            wide(),
            label(status(term), TypeStyle::Mono, Tone::Muted),
        )
    });
    tree.push(entry(FOOTER, COLUMN, padded(8, 8), row()));
    tree.push(entry(
        STATUS,
        FOOTER,
        Layout {
            width: Size::Fill(1),
            ..Layout::FIT
        },
        label(
            &format!("{} · {} rows", status(term), spans.len()),
            TypeStyle::Small,
            Tone::Muted,
        ),
    ));
    tree.push(entry(
        CLEAR,
        FOOTER,
        Layout::FIT,
        Widget::Button {
            text: String::from("Clear"),
            action: Some(String::from(CLEAR_ACTION)),
        },
    ));
    // Last, so it comes and goes without moving the others.
    if matches!(term.phase(), Phase::Continuing | Phase::Asking(_)) {
        tree.push(entry(
            CANCEL,
            FOOTER,
            Layout::FIT,
            Widget::Button {
                text: String::from("Cancel"),
                action: None,
            },
        ));
    }
    tree
}

/// The node that should take keyboard focus after `old` became `new`: the input
/// line, when it has just appeared (the window opening, or a line answered).
pub fn wants_focus(old: &[Entry], new: &[Entry]) -> Option<NodeId> {
    let has = |tree: &[Entry]| tree.iter().any(|e| e.id == INPUT);
    (has(new) && !has(old)).then_some(INPUT)
}

/// Handles one event from the window.
pub fn on_ui(term: &mut Terminal, view: &mut View, e: &UiEvent) -> Outcome {
    match e {
        UiEvent::Changed(INPUT, Value::Text(text)) => {
            view.draft = text.chars().take(MAX_LINE).collect();
            // The field already shows it, so there is nothing to send.
            view.held.clone_from(&view.draft);
            Outcome::default()
        }
        UiEvent::Submitted(INPUT) => {
            let line = core::mem::take(&mut view.draft);
            let send = match term.submit(&line, comp_terminal::Source::Typed) {
                comp_terminal::Submitted::Send(line) => Some(line),
                comp_terminal::Submitted::Refused(_) => {
                    view.draft = line;
                    None
                }
                comp_terminal::Submitted::Nothing | comp_terminal::Submitted::Local => None,
            };
            Outcome { redraw: true, send }
        }
        UiEvent::Pressed(CLEAR) => {
            term.clear();
            Outcome {
                redraw: true,
                send: None,
            }
        }
        UiEvent::Pressed(CANCEL) => Outcome {
            redraw: true,
            send: term.cancel(),
        },
        _ => Outcome::default(),
    }
}

/// What to send to bring the window to the model's tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Next {
    /// The tree, which becomes `shown` once the changes are sent.
    pub tree: Vec<Entry>,
    pub changes: Vec<Change>,
    /// The node to give keyboard focus after the changes, if any.
    pub focus: Option<NodeId>,
}

/// The changes from `shown`, the last tree sent, to the model's tree. `shown`'s
/// input field is first set to what the window holds ([`View::held`]), because
/// keys change the field without a tree being sent. Once the changes are sent,
/// call [`View::sent`].
pub fn next(shown: &mut [Entry], term: &Terminal, view: &View) -> Next {
    if let Some(Entry {
        widget: Widget::TextLine(field),
        ..
    }) = shown.iter_mut().find(|e| e.id == INPUT)
    {
        field.value.clone_from(&view.held);
    }
    let tree = build(term, view);
    Next {
        changes: diff(shown, &tree),
        focus: wants_focus(shown, &tree),
        tree,
    }
}

impl View {
    /// Notes that the window now holds the draft.
    pub fn sent(&mut self) {
        self.held.clone_from(&self.draft);
    }
}
