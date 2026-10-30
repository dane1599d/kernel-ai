//! The System monitor window on the host: golden display lists of its views,
//! drawn by `uikit` with `FixedAdvance`, the round trip from window events to
//! the call the OS is asked for, and that every state's tree is one `uikit`
//! accepts. Set `UIKIT_BLESS=1` to rewrite the goldens, and review the diff.

use comp_monitor::fake::Queue;
use comp_monitor::sample::{ModelState, ModelStatus, ProcSample, Sample};
use comp_monitor::{MAX_PROCS, Monitor, Sort};
use comp_monitor_ui::{
    self as ui, Change, Entry, REFRESH, SORT_CPU, SORT_MEMORY, SORT_NAME, SORT_PID, STOP, TABLE,
};
use std::path::PathBuf;
use uikit::a11y::Action;
use uikit::layout::FixedAdvance as M;
use uikit::widget::{UiEvent, Value, Widget};
use uikit::{Frame, Window};

const MS: u64 = 1_000_000;
const SEC: u64 = 1_000_000_000;

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

fn proc(pid: u32, name: &str, pages: u64, cpu_ns: u64) -> ProcSample {
    ProcSample {
        pid,
        name: name.into(),
        pages,
        cpu_ns,
    }
}

/// A 4-CPU machine with 1,500 of 4,096 MiB free; its processes use CPU at the
/// same rate, so the share is the same whenever it is sampled.
fn usual(seconds: u64, model: ModelStatus) -> Sample {
    Sample {
        uptime_ns: 754 * SEC + seconds * SEC,
        cpus: 4,
        free_pages: 1500 * 256,
        total_pages: 4096 * 256,
        heap_bytes: 820 * 1024,
        procs: vec![
            proc(1, "supervisor", 75, 5 * MS * seconds),
            proc(4, "llm", 600_000, 612 * MS * seconds),
            proc(7, "desktop", 10_250, 80 * MS * seconds),
            proc(9, "shell", 40, 0),
        ],
        omitted: 0,
        model,
    }
}

const ANSWERING: ModelStatus = ModelStatus {
    state: ModelState::Ready,
    tps10: 125,
};

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

    /// Two samples a second apart, so every process has a share.
    fn full() -> Self {
        let mut app = Self::new();
        app.mon.update(&usual(1, ANSWERING));
        app.mon.update(&usual(2, ANSWERING));
        app
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
fn the_first_sample_has_no_shares_and_the_model_not_yet_reported() {
    let mut app = App::new();
    app.mon.update(&usual(1, ModelStatus::UNKNOWN));
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
    // The row the monitor stops with is the OS's.
    assert!(agentabi::ROWS.iter().any(|r| r.name == "process_stop"));
}

#[test]
fn clicks_in_the_window_become_the_call_the_os_is_asked_for() {
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
    assert!(app.event(&UiEvent::Pressed(STOP)));
    let call = &app.os.calls[0];
    assert_eq!(
        (call.name, call.args.as_str()),
        ("process_stop", r#"{name: "desktop"}"#)
    );

    // A sort button sorts, and the same button reverses.
    assert!(app.event(&UiEvent::Pressed(SORT_MEMORY)));
    assert_eq!(app.mon.procs()[0].name, "llm");
    assert!(app.event(&UiEvent::Pressed(SORT_MEMORY)));
    assert_eq!(app.mon.procs()[0].name, "shell");
    assert!(app.event(&UiEvent::Pressed(SORT_NAME)));
    assert_eq!(app.mon.procs()[0].name, "desktop");
    assert!(app.event(&UiEvent::Pressed(SORT_PID)));
    assert_eq!(app.mon.procs()[0].pid, 1);

    // Refresh only redraws: the component reads a new sample after any event.
    assert!(app.event(&UiEvent::Pressed(REFRESH)));
    assert_eq!(app.os.calls.len(), 1);

    // What the window does not use changes nothing.
    assert!(!app.event(&UiEvent::Pressed(9_999)));
    assert!(!app.event(&UiEvent::Changed(TABLE, Value::Index(500))));
    assert!(!app.event(&UiEvent::CloseRequested));
}

#[test]
fn stop_with_nothing_selected_asks_nothing() {
    let mut app = App::full();
    assert!(app.event(&UiEvent::Pressed(STOP)));
    assert!(app.os.calls.is_empty());
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
    app.mon.press_sort(Sort::Name);
    step(&app, &mut shown, &mut win, 3);
    app.mon.update(&usual(3, ModelStatus::UNKNOWN));
    step(&app, &mut shown, &mut win, 4);
    app.mon.stop_selected(&mut app.os);
    app.mon.done(1, Err("the user said no".into()));
    step(&app, &mut shown, &mut win, 5);
    assert!(ui::diff(&shown, &shown).is_empty());
}

#[test]
fn a_full_process_table_is_a_window_the_tree_limits_allow() {
    let mut app = App::new();
    let procs = (0..300)
        .map(|i| {
            let name = format!("{}\u{1b}[31m{i}", "n".repeat(80));
            proc(i, &name, u64::from(i) * 977, u64::from(i) * 3 * MS)
        })
        .collect();
    let mut s = usual(1, ANSWERING);
    s.procs = procs;
    app.mon.update(&s);
    assert_eq!(app.mon.procs().len(), MAX_PROCS);
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
