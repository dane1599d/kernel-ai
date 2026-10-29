//! The monitor model on the host: what it asks the OS for, how it reads the
//! answers, how it sorts and selects, and that hostile answers change nothing
//! but what the window shows. The fake queue gives tickets from 1.

#[path = "../../../testkit/hostile.rs"]
mod hostile;

use comp_monitor::fake::Queue;
use comp_monitor::rows::{self, MAX_PROCS, NAME_CHARS};
use comp_monitor::{Monitor, Sort, Status, view};

const PROCS: &str = r#"[{pid: 1, name: "supervisor", cpu-permille: some(5), mem-kib: some(300)},
 {pid: 4, name: "llm", cpu-permille: some(612), mem-kib: some(2400000)},
 {pid: 7, name: "desktop", cpu-permille: some(80), mem-kib: some(41000)},
 {pid: 9, name: "shell", cpu-permille: none, mem-kib: none}]"#;
const MEMORY: &str = "{free-mib: 1500, total-mib: 4096, heap-kib: 820}";
const MODEL: &str = r#"{state: "answering", tokens-per-s-x10: some(125)}"#;

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
        app.answer("memory", Ok(MEMORY));
        app.answer("cpus", Ok("4"));
        app.answer("uptime_ms", Ok("754000"));
        app.answer("model_state", Ok(MODEL));
        app
    }

    fn answer(&mut self, row: &str, outcome: Result<&str, &str>) -> bool {
        let ticket = self.os.ticket_of(row).expect("the row was asked");
        let outcome = outcome.map(String::from).map_err(String::from);
        self.mon.done(&mut self.os, ticket, outcome)
    }

    fn names(&self) -> Vec<&str> {
        self.mon.procs().iter().map(|p| p.name.as_str()).collect()
    }
}

#[test]
fn a_refresh_asks_five_reads_and_none_changes_anything() {
    let mut app = App::new();
    app.mon.refresh(&mut app.os);
    let names: Vec<_> = app.os.calls.iter().map(|c| c.name).collect();
    assert_eq!(
        names,
        ["processes", "memory", "cpus", "uptime_ms", "model_state"]
    );
    assert!(app.os.calls.iter().all(|c| c.args == "{}"));
    assert!(app.mon.busy());
}

#[test]
fn the_answers_fill_the_window_with_the_busiest_first() {
    let app = App::full();
    assert!(!app.mon.busy());
    assert_eq!(app.names(), ["llm", "desktop", "supervisor", "shell"]);
    assert_eq!(app.mon.cpus(), Some(4));
    assert_eq!(app.mon.uptime_ms(), Some(754_000));
    assert_eq!(app.mon.memory().unwrap().free_mib, 1500);
    assert_eq!(app.mon.model().unwrap().state, "answering");
    assert_eq!(app.mon.model().unwrap().tokens_per_s_x10, Some(125));
    assert_eq!(app.mon.status(), &Status::Ready);
}

#[test]
fn the_rows_as_they_are_today_still_read() {
    // `processes` gives only pid and name; the other columns are unknown.
    let mut app = App::new();
    app.mon.refresh(&mut app.os);
    app.answer(
        "processes",
        Ok(r#"[{pid: 3, name: "shell"}, {pid: 1, name: "supervisor"}]"#),
    );
    app.answer("memory", Ok("{free_mib: 10, total_mib: 20, heap_kib: 5}"));
    app.answer("model_state", Err("no such row"));
    assert_eq!(app.names(), ["supervisor", "shell"]);
    assert!(
        app.mon
            .procs()
            .iter()
            .all(|p| p.cpu_permille.is_none() && p.mem_kib.is_none())
    );
    assert_eq!(app.mon.memory().unwrap().total_mib, 20);
    // No model row is not a failure.
    assert_eq!(app.mon.model(), None);
    assert_eq!(app.mon.status(), &Status::Ready);
}

#[test]
fn sorting_by_memory_names_the_top_process() {
    let mut app = App::full();
    let said = app.mon.act_sort("memory").unwrap();
    assert_eq!(app.names(), ["llm", "desktop", "supervisor", "shell"]);
    assert_eq!(
        said,
        "Sorted by memory. First: llm (pid 4), 2.2 GiB, 61.2% CPU."
    );
    assert_eq!(
        app.mon.act_sort(" Name ").unwrap(),
        "Sorted by name. First: desktop (pid 7), 40.0 MiB, 8.0% CPU."
    );
    assert_eq!(app.names(), ["desktop", "llm", "shell", "supervisor"]);
    app.mon.act_sort("pid").unwrap();
    assert_eq!(app.names(), ["supervisor", "llm", "desktop", "shell"]);
    assert!(app.mon.act_sort("colour").is_err());
    assert!(app.mon.act_sort("").is_err());
    assert_eq!(Sort::parse("MEMORY"), Some(Sort::Memory));
}

#[test]
fn a_second_press_reverses_and_unknowns_stay_last() {
    let mut app = App::full();
    app.mon.press_sort(Sort::Memory);
    assert_eq!(app.names().last(), Some(&"shell"));
    app.mon.press_sort(Sort::Memory);
    assert!(app.mon.flipped());
    // Smallest first; the process with no figure is still last.
    assert_eq!(app.names(), ["supervisor", "desktop", "llm", "shell"]);
    app.mon.press_sort(Sort::Cpu);
    assert!(!app.mon.flipped());
    assert_eq!(app.names()[0], "llm");
}

#[test]
fn selecting_by_name_or_row_keeps_the_process_through_a_refresh() {
    let mut app = App::full();
    assert_eq!(
        app.mon.act_select("DESKTOP").unwrap(),
        "Selected desktop (pid 7), 40.0 MiB, 8.0% CPU."
    );
    assert_eq!(app.mon.selected().unwrap().pid, 7);
    assert!(app.mon.act_select("nothing").is_err());
    assert!(app.mon.select_index(0));
    assert_eq!(app.mon.selected().unwrap().pid, 4);
    assert!(!app.mon.select_index(99));

    // After a refresh in another order, the same process is still selected.
    app.mon.refresh(&mut app.os);
    app.answer(
        "processes",
        Ok(r#"[{pid: 9, name: "shell"}, {pid: 4, name: "llm"}]"#),
    );
    assert_eq!(app.mon.selected().unwrap().name, "llm");
    // And when it is gone, nothing is.
    app.mon.refresh(&mut app.os);
    app.answer("processes", Ok(r#"[{pid: 9, name: "shell"}]"#));
    assert!(app.mon.selected().is_none());
    assert_eq!(app.mon.selected_index(), None);
}

#[test]
fn stopping_asks_the_os_by_name_and_only_for_a_name_it_can_give() {
    let mut app = App::full();
    app.mon.stop_selected(&mut app.os);
    assert_eq!(
        app.mon.status(),
        &Status::Failed("Pick a process to stop first.".into())
    );
    let before = app.os.calls.len();
    app.mon.act_select("llm").unwrap();
    app.mon.stop_selected(&mut app.os);
    let call = app.os.calls.last().unwrap();
    assert_eq!(
        (call.name, call.args.as_str()),
        ("process_stop", r#"{name: "llm"}"#)
    );
    assert_eq!(app.os.calls.len(), before + 1);

    // The user said no, or the row refused: the status says so, nothing else moves.
    app.answer("process_stop", Err("not allowed"));
    assert_eq!(app.mon.status(), &Status::Failed("not allowed".into()));
    assert_eq!(app.os.calls.len(), before + 1);

    // It stopped: the monitor reads everything again.
    app.mon.stop_selected(&mut app.os);
    let asked = app.os.calls.len();
    app.answer("process_stop", Ok(""));
    assert_eq!(app.os.calls.len(), asked + 5);
    assert_eq!(app.mon.status(), &Status::Done("Stopped llm.".into()));
}

#[test]
fn a_name_that_was_cut_or_changed_is_never_given_to_the_stop_row() {
    let mut app = App::new();
    app.mon.refresh(&mut app.os);
    let long = "x".repeat(NAME_CHARS + 10);
    app.answer(
        "processes",
        Ok(&format!(
            r#"[{{pid: 1, name: "{long}"}}, {{pid: 2, name: "bad\u{{1b}}name"}}, {{pid: 3, name: "q\"uote"}}]"#
        )),
    );
    for pid in [1, 2, 3] {
        let i = app.mon.procs().iter().position(|p| p.pid == pid).unwrap();
        assert!(app.mon.select_index(i));
        let before = app.os.calls.len();
        app.mon.stop_selected(&mut app.os);
        assert_eq!(app.os.calls.len(), before, "pid {pid}");
        assert!(matches!(app.mon.status(), Status::Failed(_)));
    }
    assert!(app.mon.procs().iter().all(
        |p| p.name.chars().count() <= NAME_CHARS + 1 && p.name.chars().all(|c| !c.is_control())
    ));
}

#[test]
fn an_answer_from_before_the_latest_refresh_is_dropped() {
    let mut app = App::new();
    app.mon.refresh(&mut app.os);
    let old = app.os.ticket_of("processes").unwrap();
    app.mon.refresh(&mut app.os);
    assert!(!app.mon.done(&mut app.os, old, Ok(PROCS.into())));
    assert!(app.mon.procs().is_empty());
    assert!(!app.mon.done(&mut app.os, 9_999, Ok("4".into())));
}

#[test]
fn bad_answers_say_which_row_was_wrong_and_leave_the_old_numbers() {
    let mut app = App::full();
    for (row, text, why) in [
        ("processes", "not a list", "processes: "),
        (
            "memory",
            "{free-mib: 5000, total-mib: 100, heap-kib: 1}",
            "memory: ",
        ),
        (
            "memory",
            "{free-mib: -1, total-mib: 100, heap-kib: 1}",
            "memory: ",
        ),
        ("cpus", "0", "cpus: "),
        ("cpus", "\"four\"", "cpus: "),
        ("uptime_ms", "-5", "uptime: "),
    ] {
        app.mon.refresh(&mut app.os);
        app.answer(row, Ok(text));
        let Status::Failed(msg) = app.mon.status() else {
            panic!("{row} {text}");
        };
        assert!(msg.starts_with(why), "{msg}");
    }
    assert_eq!(app.mon.cpus(), Some(4));
    assert_eq!(app.mon.memory().unwrap().total_mib, 4096);
    assert_eq!(app.mon.procs().len(), 4);
}

#[test]
fn numbers_read_the_way_a_person_would() {
    assert_eq!(view::cpu(Some(123)), "12.3%");
    assert_eq!(view::cpu(Some(1000)), "100.0%");
    assert_eq!(view::cpu(None), "-");
    assert_eq!(view::size(Some(340)), "340 KiB");
    assert_eq!(view::size(Some(1536)), "1.5 MiB");
    assert_eq!(view::size(Some(2 * 1024 * 1024)), "2.0 GiB");
    assert_eq!(view::uptime(Some(9_000)), "9s");
    assert_eq!(view::uptime(Some(432_000)), "7m 12s");
    assert_eq!(view::uptime(Some(7_500_000)), "2h 5m");
    assert_eq!(view::uptime(Some(100_000_000)), "1d 3h");
    assert_eq!(view::tokens(Some(125)), "12.5 tokens/s");
    let m = rows::parse_memory(MEMORY).unwrap();
    assert_eq!(view::used_permille(Some(m)), Some(633));
    assert_eq!(view::memory_line(Some(m)), "2596 of 4096 MiB used");
    let zero = rows::Memory {
        free_mib: 0,
        total_mib: 0,
        heap_kib: 0,
    };
    assert_eq!(view::used_permille(Some(zero)), None);
}

#[test]
fn hostile_answers_never_panic_and_stay_in_bounds() {
    let samples: [&[u8]; 3] = [PROCS.as_bytes(), MEMORY.as_bytes(), MODEL.as_bytes()];
    let mut rng = hostile::XorShift::new(0xbeef);
    for round in 0..6_000 {
        let bytes = hostile::mutate(&mut rng, samples[round % 3]);
        let text = String::from_utf8_lossy(&bytes);
        if let Ok(procs) = rows::parse_processes(&text) {
            assert!(procs.len() <= MAX_PROCS);
            for p in &procs {
                assert!(p.name.chars().count() <= NAME_CHARS + 1);
                assert!(p.name.chars().all(|c| !c.is_control()));
            }
        }
        if let Ok(m) = rows::parse_memory(&text) {
            assert!(m.free_mib <= m.total_mib);
        }
        let _ = rows::parse_model(&text);
        let _ = rows::parse_cpus(&text);
        let _ = rows::parse_uptime(&text);
    }
}

#[test]
fn a_huge_process_list_is_cut_and_a_huge_text_is_refused() {
    let list: String = (0..300)
        .map(|i| format!("{{pid: {i}, name: \"p{i}\"}}"))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        rows::parse_processes(&format!("[{list}]")).unwrap().len(),
        MAX_PROCS
    );
    let too_long = format!("[{}]", "{pid: 1, name: \"x\"}, ".repeat(10_000));
    assert!(rows::parse_processes(&too_long).is_err());
    let deep = format!("{}1{}", "[".repeat(5_000), "]".repeat(5_000));
    assert!(rows::parse_processes(&deep).is_err());
}
