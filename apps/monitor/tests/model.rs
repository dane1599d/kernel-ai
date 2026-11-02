//! The monitor model on the host: how it reads a sample, how two samples make a
//! CPU share, how it sorts and selects, what the status line says of a sample it could not read, and that
//! odd numbers change nothing but what the window shows.

#[path = "../../../testkit/hostile.rs"]
mod hostile;

use comp_monitor::sample::{ModelState, ModelStatus, ProcSample, Sample};
use comp_monitor::{MAX_PROCS, Monitor, NAME_CHARS, Sort, Status, view};

const MS: u64 = 1_000_000;
const SEC: u64 = 1_000_000_000;

const READY: ModelStatus = ModelStatus {
    state: ModelState::Ready,
    tps10: 125,
};

fn proc(pid: u32, name: &str, pages: u64, cpu_ns: u64) -> ProcSample {
    ProcSample {
        pid,
        name: name.into(),
        pages,
        cpu_ns,
    }
}

/// A 4-CPU machine with 1,500 of 4,096 MiB free, `uptime_ns` after boot.
fn sample(uptime_ns: u64, procs: Vec<ProcSample>) -> Sample {
    Sample {
        uptime_ns,
        cpus: 4,
        free_pages: 1500 * 256,
        total_pages: 4096 * 256,
        heap_bytes: 820 * 1024,
        procs,
        omitted: 0,
        model: READY,
    }
}

/// The usual four processes, with CPU time that grows by `step` a second.
fn usual(uptime_ns: u64, step: u64) -> Sample {
    let t = uptime_ns / SEC;
    sample(
        uptime_ns,
        vec![
            proc(1, "supervisor", 75, 5 * MS * t),
            proc(4, "llm", 600_000, step * t),
            proc(7, "desktop", 10_250, 80 * MS * t),
            proc(9, "shell", 40, 0),
        ],
    )
}

fn names(mon: &Monitor) -> Vec<&str> {
    mon.procs().iter().map(|p| p.name.as_str()).collect()
}

/// A monitor that has seen the usual machine at 1 s and 2 s.
fn running() -> Monitor {
    let mut mon = Monitor::new();
    mon.update(&usual(SEC, 612 * MS));
    mon.update(&usual(2 * SEC, 612 * MS));
    mon
}

#[test]
fn a_sample_fills_the_window_and_the_first_has_no_shares() {
    let mut mon = Monitor::new();
    mon.update(&usual(SEC, 612 * MS));
    assert_eq!(mon.cpus(), Some(4));
    assert_eq!(mon.uptime_ms(), Some(1000));
    let memory = mon.memory().unwrap();
    assert_eq!(
        (memory.free_mib, memory.total_mib, memory.heap_kib),
        (1500, 4096, 820)
    );
    assert_eq!(mon.model(), READY);
    let llm = mon.procs().iter().find(|p| p.name == "llm").unwrap();
    // Pages are 4 KiB; the share needs a second sample.
    assert_eq!((llm.mem_kib, llm.cpu_permille), (Some(2_400_000), None));
    assert_eq!(mon.status(), &Status::Ready);
}

#[test]
fn two_samples_make_each_processes_cpu_share() {
    let mon = running();
    let share = |name: &str| {
        mon.procs()
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .cpu_permille
    };
    // dcpu_ns * 1000 / duptime_ns: 612 ms of CPU in one second is 612.
    assert_eq!(share("llm"), Some(612));
    assert_eq!(share("desktop"), Some(80));
    assert_eq!(share("supervisor"), Some(5));
    assert_eq!(share("shell"), Some(0));
    assert_eq!(names(&mon), ["llm", "desktop", "supervisor", "shell"]);
}

#[test]
fn a_share_is_over_the_time_between_samples_and_no_more_than_the_cpus_have() {
    let mut mon = Monitor::new();
    mon.update(&sample(
        10 * SEC,
        vec![proc(4, "llm", 1, 0), proc(5, "hog", 1, 0)],
    ));
    // Half a second later: 100 ms of CPU is 200; nine seconds of it, on four
    // CPUs, cannot be more than 4,000.
    mon.update(&sample(
        10 * SEC + SEC / 2,
        vec![proc(4, "llm", 1, 100 * MS), proc(5, "hog", 1, 9 * SEC)],
    ));
    let share = |name: &str| {
        mon.procs()
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .cpu_permille
    };
    assert_eq!(share("llm"), Some(200));
    assert_eq!(share("hog"), Some(4000));
}

#[test]
fn two_reads_in_the_same_instant_keep_the_older_baseline() {
    let mut mon = Monitor::new();
    mon.update(&usual(SEC, 500 * MS));
    // Same uptime again: no time passed, so no share, and the baseline stays.
    mon.update(&usual(SEC, 500 * MS));
    assert!(mon.procs().iter().all(|p| p.cpu_permille.is_none()));
    // A second later the share is against the first sample.
    mon.update(&usual(2 * SEC, 500 * MS));
    let llm = mon.procs().iter().find(|p| p.name == "llm").unwrap();
    assert_eq!(llm.cpu_permille, Some(500));
}

#[test]
fn a_new_process_under_an_old_pid_or_a_clock_gone_back_gives_no_share() {
    let mut mon = running();
    // llm restarted: same pid and name, less CPU time than before.
    mon.update(&sample(3 * SEC, vec![proc(4, "llm", 1, 10 * MS)]));
    assert_eq!(mon.procs()[0].cpu_permille, None);
    // The pid was reused by another program.
    mon.update(&sample(4 * SEC, vec![proc(4, "other", 1, 900 * MS)]));
    assert_eq!(mon.procs()[0].cpu_permille, None);
    // Time went backwards (a reboot): no share, and a fresh baseline.
    mon.update(&sample(SEC, vec![proc(4, "other", 1, 50 * MS)]));
    assert_eq!(mon.procs()[0].cpu_permille, None);
    mon.update(&sample(2 * SEC, vec![proc(4, "other", 1, 150 * MS)]));
    assert_eq!(mon.procs()[0].cpu_permille, Some(100));
}

#[test]
fn what_the_kernel_could_not_say_reads_as_unknown() {
    let mut mon = Monitor::new();
    let mut s = usual(SEC, 500 * MS);
    s.cpus = 0;
    s.total_pages = 0;
    s.model = ModelStatus::UNKNOWN;
    s.omitted = 3;
    mon.update(&s);
    assert_eq!((mon.cpus(), mon.memory()), (None, None));
    assert_eq!(mon.model().state, ModelState::Unknown);
    assert_eq!(mon.omitted(), 3);
    // More free than total is not a memory.
    s.total_pages = 10;
    s.free_pages = 20;
    mon.update(&s);
    assert_eq!(mon.memory(), None);
}

#[test]
fn sorting_by_memory_names_the_top_process() {
    let mut mon = running();
    let said = mon.act_sort("memory").unwrap();
    assert_eq!(names(&mon), ["llm", "desktop", "supervisor", "shell"]);
    assert_eq!(
        said,
        "Sorted by memory. First: llm (pid 4), 2.2 GiB, 61.2% CPU."
    );
    assert_eq!(
        mon.act_sort(" Name ").unwrap(),
        "Sorted by name. First: desktop (pid 7), 40.0 MiB, 8.0% CPU."
    );
    assert_eq!(names(&mon), ["desktop", "llm", "shell", "supervisor"]);
    mon.act_sort("pid").unwrap();
    assert_eq!(names(&mon), ["supervisor", "llm", "desktop", "shell"]);
    assert!(mon.act_sort("colour").is_err());
    assert!(mon.act_sort("").is_err());
    assert_eq!(Sort::parse("MEMORY"), Some(Sort::Memory));
}

#[test]
fn a_second_press_reverses_and_unknowns_stay_last() {
    let mut mon = Monitor::new();
    // The first sample has no shares: every CPU figure is unknown.
    mon.update(&usual(SEC, 612 * MS));
    mon.press_sort(Sort::Cpu);
    assert_eq!(names(&mon), ["supervisor", "llm", "desktop", "shell"]);
    let mut mon = running();
    mon.press_sort(Sort::Memory);
    assert_eq!(mon.procs()[0].name, "llm");
    mon.press_sort(Sort::Memory);
    assert!(mon.flipped());
    // Smallest first.
    assert_eq!(names(&mon), ["shell", "supervisor", "desktop", "llm"]);
    mon.press_sort(Sort::Cpu);
    assert!(!mon.flipped());
    assert_eq!(names(&mon)[0], "llm");
}

#[test]
fn selecting_by_name_or_row_keeps_the_process_through_a_new_sample() {
    let mut mon = running();
    assert_eq!(
        mon.act_select("DESKTOP").unwrap(),
        "Selected desktop (pid 7), 40.0 MiB, 8.0% CPU."
    );
    assert_eq!(mon.selected().unwrap().pid, 7);
    assert!(mon.act_select("nothing").is_err());
    assert!(mon.select_index(0));
    assert_eq!(mon.selected().unwrap().pid, 4);
    assert!(!mon.select_index(99));

    // After a sample in another order, the same process is still selected.
    mon.update(&sample(
        3 * SEC,
        vec![proc(9, "shell", 40, 0), proc(4, "llm", 600_000, 2 * SEC)],
    ));
    assert_eq!(mon.selected().unwrap().name, "llm");
    assert_eq!(mon.selected_index(), Some(0));
    // And when it is gone, nothing is.
    mon.update(&sample(4 * SEC, vec![proc(9, "shell", 40, 0)]));
    assert!(mon.selected().is_none());
    assert_eq!(mon.selected_index(), None);
}

#[test]
fn a_name_the_kernel_gave_is_shown_cut_and_without_control_characters() {
    let mut mon = Monitor::new();
    let long = "x".repeat(NAME_CHARS + 10);
    mon.update(&sample(
        SEC,
        vec![
            proc(1, &long, 1, 0),
            proc(2, "bad\u{1b}name", 1, 0),
            proc(3, "q\"uote", 1, 0),
            proc(4, "back\\slash", 1, 0),
        ],
    ));
    assert_eq!(mon.procs().len(), 4);
    assert!(mon.procs().iter().all(|p| {
        p.name.chars().count() <= NAME_CHARS + 1 && p.name.chars().all(|c| !c.is_control())
    }));
    // Quotes and backslashes are text, kept as they came.
    assert!(mon.procs().iter().any(|p| p.name == "q\"uote"));
    assert!(mon.procs().iter().any(|p| p.name == "back\\slash"));
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
    let mon = running();
    assert_eq!(view::used_permille(mon.memory()), Some(633));
    assert_eq!(view::memory_line(mon.memory()), "2596 of 4096 MiB used");
}

#[test]
fn any_numbers_at_all_leave_a_sane_window() {
    let mut rng = hostile::XorShift::new(0xbeef);
    let extreme = |rng: &mut hostile::XorShift| match rng.below(4) {
        0 => 0,
        1 => u64::MAX,
        2 => hostile::INTERESTING[rng.below(hostile::INTERESTING.len())],
        _ => rng.next() % 5_000_000_000,
    };
    let mut mon = Monitor::new();
    for _ in 0..3_000 {
        let procs = (0..rng.below(30))
            .map(|_| {
                let long = "n".repeat(300);
                let name = ["llm", "a\u{7}b", "", long.as_str()][rng.below(4)];
                proc(
                    rng.below(40) as u32,
                    name,
                    extreme(&mut rng),
                    extreme(&mut rng),
                )
            })
            .collect();
        let s = Sample {
            uptime_ns: extreme(&mut rng),
            cpus: rng.below(9) as u32,
            free_pages: extreme(&mut rng),
            total_pages: extreme(&mut rng),
            heap_bytes: extreme(&mut rng),
            procs,
            omitted: rng.next() as u32,
            model: READY,
        };
        mon.update(&s);
        assert!(mon.procs().len() <= MAX_PROCS);
        for p in mon.procs() {
            // A share is at most the CPUs the sample had.
            let most = s.cpus.max(1) * 1000;
            assert!(p.cpu_permille.is_none_or(|c| c <= most), "{p:?}");
            assert!(p.name.chars().all(|c| !c.is_control()));
        }
        if let Some(m) = mon.memory() {
            assert!(m.free_mib <= m.total_mib);
        }
        match rng.below(5) {
            0 => mon.press_sort(Sort::ALL[rng.below(4)]),
            1 => drop(mon.select_index(rng.below(35))),
            _ => {}
        }
        // The selection, if any, is a row of the table.
        assert_eq!(mon.selected().is_some(), mon.selected_index().is_some());
    }
}

#[test]
fn a_sample_the_host_wrote_is_the_sample_the_monitor_reads() {
    // comphost writes `Sample::encode`; the component parses it with
    // `Sample::parse` and hands it to the model.
    let mut mon = Monitor::new();
    for seconds in [1, 2] {
        let wire = usual(seconds * SEC, 612 * MS).encode();
        let sample = Sample::parse(&wire).expect("what the host writes parses");
        mon.update(&sample);
    }
    assert_eq!(names(&mon), names(&running()));
    assert_eq!(mon.model(), READY);
    assert!(
        mon.procs()
            .iter()
            .any(|p| p.name == "llm" && p.cpu_permille.is_some())
    );
}

#[test]
fn an_unreadable_sample_is_said_and_the_next_good_one_clears_it() {
    let mut mon = running();
    let before = names(&mon).len();
    mon.no_reading("the system could not be read");
    assert!(matches!(mon.status(), Status::Failed(m) if m.contains("could not be read")));
    // The last picture stays.
    assert_eq!(names(&mon).len(), before);
    mon.update(&usual(3 * SEC, 612 * MS));
    assert_eq!(mon.status(), &Status::Ready);

    // Odd text is data.
    mon.no_reading("a\u{1b}[31m\u{202e} very long ".repeat(20).as_str());
    let Status::Failed(text) = mon.status() else {
        panic!("{:?}", mon.status());
    };
    assert!(text.chars().all(|c| !c.is_control()) && text.chars().count() <= 120);
}
