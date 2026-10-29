//! The System monitor's window on `kai:ui` (ADR 0005 section 4): the machine's
//! numbers, a sort row, the process table, the model's line and a Stop button.
//! The tree is rebuilt whole from the model after each event and [`diff`]ed
//! against the last one, as Notes' is.
//!
//! The sort buttons are bound to the app's own `monitor_sort`, and Stop to the
//! OS's `process_stop`, so the user reads, the agent sees and the log says the
//! same words. Process names are the system's data and are drawn as text.

#![no_std]

extern crate alloc;

mod tree;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use comp_monitor::{Monitor, Os, Sort, Status, tame, view};
pub use tree::{Change, Entry, diff};
use tree::{column, entry, label, row, wide};
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
pub const ACTIONS: NodeId = 14;
pub const STOP: NodeId = 15;
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
    match mon.model() {
        None => String::from("Model: not reported"),
        Some(m) => match m.tokens_per_s_x10 {
            Some(t) => format!("Model: {}, {}", m.state, view::tokens(Some(t))),
            None => format!("Model: {}", m.state),
        },
    }
}

fn status(mon: &Monitor) -> (String, Tone) {
    match mon.status() {
        Status::Failed(why) => (why.clone(), Tone::Error),
        Status::Done(what) => (what.clone(), Tone::Ok),
        Status::Ready if mon.busy() => (String::from("Reading…"), Tone::Muted),
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
        Some(p) if p.exact => format!("Stops every process named {}", tame(&p.name, 32)),
        Some(_) => String::from("This name cannot be stopped from here"),
        None => String::from("Pick a process to stop it"),
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
        entry(ACTIONS, COLUMN, wide(), row()),
        entry(
            STOP,
            ACTIONS,
            Layout::FIT,
            Widget::Button {
                text: String::from("Stop"),
                action: Some(String::from("process_stop")),
            },
        ),
        entry(
            HINT,
            ACTIONS,
            wide(),
            label(&hint, TypeStyle::Small, Tone::Muted)
        ),
        entry(STATUS, COLUMN, wide(), label(&text, TypeStyle::Small, tone)),
    ]
}

/// Handles one event from the window; whether the tree may have changed.
pub fn on_ui(mon: &mut Monitor, os: &mut dyn Os, e: &UiEvent) -> bool {
    match e {
        UiEvent::Pressed(SORT_CPU) => mon.press_sort(Sort::Cpu),
        UiEvent::Pressed(SORT_MEMORY) => mon.press_sort(Sort::Memory),
        UiEvent::Pressed(SORT_NAME) => mon.press_sort(Sort::Name),
        UiEvent::Pressed(SORT_PID) => mon.press_sort(Sort::Pid),
        UiEvent::Pressed(REFRESH) => mon.refresh(os),
        UiEvent::Pressed(STOP) => mon.stop_selected(os),
        UiEvent::Changed(TABLE, Value::Index(i)) => return mon.select_index(*i as usize),
        _ => return false,
    }
    true
}
