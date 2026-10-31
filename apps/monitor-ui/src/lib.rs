//! The System monitor's window on `kai:ui` (ADR 0005 section 4): the machine's
//! numbers, a sort row, the process table, the model's line and a hint.
//! The tree is rebuilt whole from the model after each event and [`diff`]ed
//! against the last one, as Notes' is.
//!
//! The sort buttons are bound to the app's own `monitor_sort`, so the user reads,
//! the agent sees and the log says the same words. There is no Stop button:
//! stopping a process is the OS's `process_stop`, which asks, and no app may
//! declare it as an effect, so the hint says to ask Kai. Process names are the
//! system's data and are drawn as text.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use comp_monitor::{Monitor, Sort, Status, tame, view};
pub use comp_ui_bind::tree::{Change, Entry, diff};
use comp_ui_bind::tree::{column, entry, label, row, wide};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Layout, NodeId, ROOT, UiEvent, Value, Widget};

pub const COLUMN: NodeId = 1;
pub const SUMMARY: NodeId = 2;
pub const CPUS: NodeId = 3;
pub const UPTIME: NodeId = 4;
pub const MEMORY: NodeId = 5;
pub const BAR: NodeId = 6;
pub const SORT_CPU: NodeId = 7;
pub const SORT_MEMORY: NodeId = 8;
pub const SORT_NAME: NodeId = 9;
pub const SORT_PID: NodeId = 10;
pub const REFRESH: NodeId = 11;
pub const TABLE: NodeId = 12;
pub const MODEL: NodeId = 13;
pub const HINT: NodeId = 16;
pub const STATUS: NodeId = 17;
/// The window's size when it opens, in logical px.
pub const SIZE: (u16, u16) = (640, 440);
/// The window's title.
pub const TITLE: &str = "System Monitor";

fn sort_button(mon: &Monitor, sort: Sort) -> Widget {
    let name = match sort {
        Sort::Cpu => "CPU",
        Sort::Memory => "Memory",
        Sort::Name => "Name",
        Sort::Pid => "PID",
    };
    let text = match mon.sort() == sort {
        false => String::from(name),
        true => {
            let biggest_first = sort.default_biggest_first() != mon.flipped();
            let order = match (sort, biggest_first) {
                (Sort::Name, true) => "Z-A",
                (Sort::Name, false) => "A-Z",
                (_, true) => "high-low",
                (_, false) => "low-high",
            };
            format!("{name} {order}")
        }
    };
    Widget::Button {
        text,
        action: Some(String::from("monitor_sort")),
    }
}

fn model_line(mon: &Monitor) -> String {
    let m = mon.model();
    match m.tps10 {
        0 => format!("Model: {}", view::model_word(m.state)),
        t => format!(
            "Model: {}, {}",
            view::model_word(m.state),
            view::tokens(Some(t))
        ),
    }
}

fn status(mon: &Monitor) -> (String, Tone) {
    match mon.status() {
        Status::Failed(why) => (why.clone(), Tone::Error),
        Status::Ready => (format!("{} processes", mon.procs().len()), Tone::Muted),
    }
}

/// The whole tree for this model, parents before children.
pub fn build(mon: &Monitor) -> Vec<Entry> {
    let rows: Vec<Vec<String>> = mon
        .procs()
        .iter()
        .map(|p| {
            vec![
                format!("{}", p.pid),
                p.name.clone(),
                view::cpu(p.cpu_permille),
                view::size(p.mem_kib),
            ]
        })
        .collect();
    let hint = match mon.selected() {
        Some(p) => format!("To stop {}, ask Kai; it asks you first", tame(&p.name, 32)),
        None => String::from("Pick a process; to stop it, ask Kai"),
    };
    let (text, tone) = status(mon);
    alloc::vec![
        entry(
            COLUMN,
            ROOT,
            column(12, 8),
            Widget::Stack { axis: Axis::Column }
        ),
        entry(SUMMARY, COLUMN, wide(), row()),
        entry(
            CPUS,
            SUMMARY,
            Layout::FIT,
            label(
                &format!(
                    "CPUs: {}",
                    mon.cpus().map_or(String::from("-"), |n| format!("{n}"))
                ),
                TypeStyle::Body,
                Tone::Normal,
            ),
        ),
        entry(
            UPTIME,
            SUMMARY,
            wide(),
            label(
                &format!("Up {}", view::uptime(mon.uptime_ms())),
                TypeStyle::Body,
                Tone::Muted,
            ),
        ),
        entry(
            MEMORY,
            COLUMN,
            wide(),
            Widget::Progress {
                label: format!("Memory: {}", view::memory_line(mon.memory())),
                done: view::used_permille(mon.memory()),
            },
        ),
        entry(BAR, COLUMN, wide(), row()),
        entry(SORT_CPU, BAR, Layout::FIT, sort_button(mon, Sort::Cpu)),
        entry(
            SORT_MEMORY,
            BAR,
            Layout::FIT,
            sort_button(mon, Sort::Memory)
        ),
        entry(SORT_NAME, BAR, Layout::FIT, sort_button(mon, Sort::Name)),
        entry(SORT_PID, BAR, Layout::FIT, sort_button(mon, Sort::Pid)),
        entry(
            REFRESH,
            BAR,
            Layout::FIT,
            Widget::Button {
                text: String::from("Refresh"),
                action: None,
            },
        ),
        entry(
            TABLE,
            COLUMN,
            Layout::FILL,
            Widget::Table {
                label: String::from("Processes"),
                columns: vec![
                    String::from("PID"),
                    String::from("Name"),
                    String::from("CPU"),
                    String::from("Memory"),
                ],
                rows,
                selected: mon.selected_index().map(|i| i as u32),
            },
        ),
        entry(
            MODEL,
            COLUMN,
            wide(),
            label(&model_line(mon), TypeStyle::Body, Tone::Normal),
        ),
        entry(
            HINT,
            COLUMN,
            wide(),
            label(&hint, TypeStyle::Small, Tone::Muted)
        ),
        entry(STATUS, COLUMN, wide(), label(&text, TypeStyle::Small, tone)),
    ]
}

/// Handles one event from the window; whether the tree may have changed.
pub fn on_ui(mon: &mut Monitor, e: &UiEvent) -> bool {
    match e {
        UiEvent::Pressed(SORT_CPU) => mon.press_sort(Sort::Cpu),
        UiEvent::Pressed(SORT_MEMORY) => mon.press_sort(Sort::Memory),
        UiEvent::Pressed(SORT_NAME) => mon.press_sort(Sort::Name),
        UiEvent::Pressed(SORT_PID) => mon.press_sort(Sort::Pid),
        // The component reads a new sample after every event, so there is
        // nothing for the model to do: the press only redraws.
        UiEvent::Pressed(REFRESH) => {}
        UiEvent::Changed(TABLE, Value::Index(i)) => return mon.select_index(*i as usize),
        _ => return false,
    }
    true
}
