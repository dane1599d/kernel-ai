//! The System monitor window on the host: golden display lists of its views,
//! drawn by `uikit` with `FixedAdvance`, the round trip from window events to
//! the calls the OS is asked for, and that every state's tree is one `uikit`
//! accepts. Set `UIKIT_BLESS=1` to rewrite the goldens, and review the diff.

use comp_monitor::Monitor;
use comp_monitor::fake::Queue;
use comp_monitor_ui::{
    self as ui, Change, Entry, REFRESH, SORT_CPU, SORT_MEMORY, SORT_NAME, SORT_PID, STOP, TABLE,
};
use std::path::PathBuf;
use uikit::a11y::Action;
use uikit::layout::FixedAdvance as M;
use uikit::widget::{UiEvent, Value, Widget};
use uikit::{Frame, Window};

const PROCS: &str = r#"[{pid: 1, name: "supervisor", cpu-permille: some(5), mem-kib: some(300)},
 {pid: 4, name: "llm", cpu-permille: some(612), mem-kib: some(2400000)},
 {pid: 7, name: "desktop", cpu-permille: some(80), mem-kib: some(41000)},
 {pid: 9, name: "shell", cpu-permille: none, mem-kib: none}]"#;

fn golden(name: &str, text: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.txt"));
    if std::env::var_os("UIKIT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; run with UIKIT_BLESS=1", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(text, want, "{name} differs from {}", path.display());
}

struct App {
    mon: Monitor,
    os: Queue,
}

impl App {
    fn new() -> Self {
        Self {
            mon: Monitor::new(),
            os: Queue::default(),
        }
    }

    /// A refresh, answered with the usual numbers.
    fn full() -> Self {
        let mut app = Self::new();
        app.mon.refresh(&mut app.os);
        app.answer("processes", Ok(PROCS));
        app.answer(
            "memory",
            Ok("{free-mib: 1500, total-mib: 4096, heap-kib: 820}"),
        );
        app.answer("cpus", Ok("4"));
        app.answer("uptime_ms", Ok("754000"));
        app.answer(
            "model_state",
            Ok(r#"{state: "answering", tokens-per-s-x10: some(125)}"#),
        );
        app
    }

    fn answer(&mut self, row: &str, outcome: Result<&str, &str>) {
        let ticket = self.os.ticket_of(row).unwrap();
        let outcome = outcome.map(String::from).map_err(String::from);
        self.mon.done(&mut self.os, ticket, outcome);
    }

    fn tree(&self) -> Vec<Entry> {
        ui::build(&self.mon)
    }

    fn event(&mut self, e: &UiEvent) -> bool {
        ui::on_ui(&mut self.mon, &mut self.os, e)
    }
}

/// Sends `changes` to a window, as comphost will through `kai:ui/view`.
fn apply(w: &mut Window, changes: &[Change]) {
    for c in changes {
        match c {
            Change::Put(e) => w.put(e.id, e.parent, e.layout, e.widget.clone()).unwrap(),
            Change::Remove(id) => w.remove(*id).unwrap(),
        }
    }
}

fn window(tree: &[Entry]) -> Window {
    let (w, h) = ui::SIZE;
    let mut win = Window::new(ui::TITLE, w, h);
    apply(&mut win, &ui::diff(&[], tree));
    win
}

fn frame(tree: &[Entry]) -> Frame {
    window(tree).commit(&M)
}

fn a11y(f: &Frame) -> String {
    f.a11y
        .iter()
        .map(|n| {
            let value = n
                .value
                .as_ref()
                .map_or(String::new(), |v| format!(" = {v:?}"));
            let runs = n
                .bound
                .as_ref()
                .map_or(String::new(), |b| format!(" runs {b}"));
            format!("[{}] {} {:?}{value}{runs}\n", n.id, n.role.name(), n.name)
        })
        .collect()
}

#[test]
fn the_busy_machine() {
    let f = frame(&App::full().tree());
    golden("full", &f.list.to_string());
    golden("full-a11y", &a11y(&f));
}

#[test]
fn a_process_selected_and_sorted_by_memory() {
    let mut app = App::full();
    app.mon.act_sort("memory").unwrap();
    app.mon.act_select("llm").unwrap();
    let f = frame(&app.tree());
    golden("selected", &f.list.to_string());
    golden("selected-a11y", &a11y(&f));
}

#[test]
fn the_rows_as_they_are_today() {
    let mut app = App::new();
    app.mon.refresh(&mut app.os);
    app.answer(
        "processes",
        Ok(r#"[{pid: 3, name: "shell"}, {pid: 1, name: "supervisor"}]"#),
    );
    app.answer("model_state", Err("no such row"));
    let f = frame(&app.tree());
    golden("bare", &f.list.to_string());
    golden("bare-a11y", &a11y(&f));
}

#[test]
fn the_buttons_are_bound_to_the_rows_they_run() {
    let tree = App::full().tree();
    let bound = |id| match &tree.iter().find(|e| e.id == id).unwrap().widget {
        Widget::Button { action, .. } => action.clone(),
        other => panic!("{other:?}"),
    };
    for sort in [SORT_CPU, SORT_MEMORY, SORT_NAME, SORT_PID] {
        assert_eq!(bound(sort).as_deref(), Some("monitor_sort"));
    }
    assert_eq!(bound(STOP).as_deref(), Some("process_stop"));
    assert_eq!(bound(REFRESH), None);
    // The rows the monitor reads and stops with are the OS's.
    for name in ["processes", "memory", "cpus", "uptime_ms", "process_stop"] {
        assert!(agentabi::ROWS.iter().any(|r| r.name == name), "{name}");
    }
}

#[test]
fn clicks_in_the_window_become_the_calls_the_os_is_asked_for() {
    let mut app = App::full();
    let old = app.tree();
    let mut win = window(&old);
    win.commit(&M);

    // The user clicks the second row, then Stop.
    let picked = win
        .act(TABLE, Action::SetValue, Some(Value::Index(1)), &M)
        .unwrap();
    for e in &picked {
        assert!(app.event(e));
    }
    assert_eq!(app.mon.selected().unwrap().name, "desktop");
    let before = app.os.calls.len();
    assert!(app.event(&UiEvent::Pressed(STOP)));
    let call = app.os.calls.last().unwrap();
    assert_eq!(app.os.calls.len(), before + 1);
    assert_eq!(
        (call.name, call.args.as_str()),
        ("process_stop", r#"{name: "desktop"}"#)
    );

    // A sort button sorts, and the same button reverses.
    assert!(app.event(&UiEvent::Pressed(SORT_MEMORY)));
    assert_eq!(app.mon.procs()[0].name, "llm");
    assert!(app.event(&UiEvent::Pressed(SORT_MEMORY)));
    assert_eq!(app.mon.procs()[0].name, "supervisor");
    assert!(app.event(&UiEvent::Pressed(SORT_NAME)));
    assert_eq!(app.mon.procs()[0].name, "desktop");
    assert!(app.event(&UiEvent::Pressed(SORT_PID)));
    assert_eq!(app.mon.procs()[0].pid, 1);

    // Refresh asks the five reads again.
    let before = app.os.calls.len();
    assert!(app.event(&UiEvent::Pressed(REFRESH)));
    assert_eq!(app.os.calls.len(), before + 5);

    // What the window does not use changes nothing.
    assert!(!app.event(&UiEvent::Pressed(9_999)));
    assert!(!app.event(&UiEvent::Changed(TABLE, Value::Index(500))));
    assert!(!app.event(&UiEvent::CloseRequested));
}

#[test]
fn stop_with_nothing_selected_asks_nothing() {
    let mut app = App::full();
    let before = app.os.calls.len();
    assert!(app.event(&UiEvent::Pressed(STOP)));
    assert_eq!(app.os.calls.len(), before);
    let tree = app.tree();
    let status = tree.iter().find(|e| e.id == ui::STATUS).unwrap();
    assert!(
        matches!(&status.widget, Widget::Label { text, .. } if text.contains("Pick a process"))
    );
}

#[test]
fn every_state_is_a_tree_the_window_accepts_and_the_diff_sends_less() {
    let mut app = App::new();
    let mut shown: Vec<Entry> = Vec::new();
    let mut win = window(&shown);
    let step = |app: &App, shown: &mut Vec<Entry>, win: &mut Window, i: usize| {
        let new = app.tree();
        let changes = ui::diff(shown, &new);
        if i > 0 {
            assert!(changes.len() < new.len(), "step {i} sent the whole tree");
        }
        apply(win, &changes);
        assert_eq!(win.commit(&M).list, frame(&new).list, "step {i}");
        *shown = new;
    };
    step(&app, &mut shown, &mut win, 0);
    app = App::full();
    step(&app, &mut shown, &mut win, 1);
    app.mon.act_select("llm").unwrap();
    step(&app, &mut shown, &mut win, 2);
    app.mon.press_sort(comp_monitor::Sort::Name);
    step(&app, &mut shown, &mut win, 3);
    app.mon.refresh(&mut app.os);
    app.answer("memory", Err("the kernel went away"));
    step(&app, &mut shown, &mut win, 4);
    assert!(ui::diff(&shown, &shown).is_empty());
}

#[test]
fn a_full_process_table_is_a_window_the_tree_limits_allow() {
    let mut app = App::new();
    app.mon.refresh(&mut app.os);
    let list: Vec<String> = (0..300)
        .map(|i| {
            format!(
                "{{pid: {i}, name: \"{}\\u{{1b}}[31m{i}\", cpu-permille: some({}), mem-kib: some({})}}",
                "n".repeat(80),
                i * 3 % 1000,
                i * 977
            )
        })
        .collect();
    app.answer("processes", Ok(&format!("[{}]", list.join(", "))));
    assert_eq!(app.mon.procs().len(), comp_monitor::rows::MAX_PROCS);
    let tree = app.tree();
    // A real window takes it, and nothing it shows has a control character.
    let f = frame(&tree);
    assert!(!f.list.to_string().contains('\u{1b}'));
    let Widget::Table { rows, .. } = &tree.iter().find(|e| e.id == TABLE).unwrap().widget else {
        panic!("a table");
    };
    assert!(
        rows.iter()
            .flatten()
            .all(|c| c.chars().all(|c| !c.is_control()))
    );
}
