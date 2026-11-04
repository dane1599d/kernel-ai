//! `kai-goals 1` read as hostile: a file that comes back from disk may be damaged
//! or written by someone else, so anything out of shape refuses the whole file, and
//! anything accepted writes back as it was read.

use goalcore::{Budget, Goals, MAX_GOALS, Presence, Proposal, Scope, Spec, StepDone, Tier};

const T0: u64 = 1_000_000;
const HOUR: u64 = 3_600_000;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn spec() -> Spec {
    Spec {
        text: String::from("tidy the reports folder"),
        by_ms: Some(T0 + 8 * HOUR),
        scope: Scope {
            rows: strings(&["fs_list", "fs_trash", "notes_*"]),
            folders: strings(&["reports"]),
        },
        budget: Budget {
            steps: 6,
            tokens: 20_000,
            wall_ms: 10 * HOUR,
        },
        checkins: vec![T0 + 2 * HOUR],
        away_ok: true,
    }
}

fn step(row: &str, target: Option<&str>, tier: Tier) -> Proposal {
    Proposal {
        row: row.to_string(),
        target: target.map(str::to_string),
        tier,
        tokens_est: 100,
    }
}

/// A file with a goal in each state that has its own lines.
fn good() -> String {
    let mut goals = Goals::new();
    let finished = goals.set(spec(), T0).unwrap();
    let waiting = goals.set(spec(), T0).unwrap();
    let running = goals.set(spec(), T0).unwrap();
    let declined = goals.set(spec(), T0).unwrap();
    let _proposed = goals.set(spec(), T0).unwrap();
    for id in [finished, waiting, running] {
        goals.approve(id, T0 + 1).unwrap();
    }
    let goal = goals.get_mut(finished).unwrap();
    goal.plan(2, "list it");
    goal.propose(
        T0 + 2,
        step("fs_list", Some("reports"), Tier::Run),
        Presence::Away,
    )
    .unwrap();
    goal.finish_step(StepDone {
        entry: Some(7),
        tokens: 300,
        failed: None,
    })
    .unwrap();
    goal.complete(T0 + 3, "listed").unwrap();
    let goal = goals.get_mut(waiting).unwrap();
    let ask = step("fs_trash", Some("reports/old.txt"), Tier::Ask);
    goal.propose(T0 + 4, ask, Presence::Away).unwrap();
    let goal = goals.get_mut(running).unwrap();
    goal.propose(T0 + 5, step("notes_add", None, Tier::Run), Presence::Here)
        .unwrap();
    goals.get_mut(declined).unwrap().cancel(T0 + 6).unwrap();
    goals.to_text()
}

fn refused(text: &str) -> String {
    match Goals::parse(text) {
        Ok(_) => panic!("accepted:\n{text}"),
        Err(why) => why,
    }
}

#[test]
fn the_good_file_reads_and_writes_back_the_same() {
    let text = good();
    assert_eq!(Goals::parse(&text).unwrap().to_text(), text);
}

#[test]
fn a_file_out_of_shape_is_refused_whole() {
    let text = good();
    assert!(refused("").contains("newline"));
    assert!(refused("kai-goals 1").contains("newline"));
    assert!(refused("kai-goals 2\nids\t1\n").contains("line 1"));
    assert!(refused("kai-goals 1\n").contains("ids"));
    assert!(refused(&text.replace('\n', "\r\n")).contains("carriage return"));
    assert!(refused(&text[..text.len() - 1]).contains("newline"));
    assert!(refused(&format!("{text}\n")).contains("expected `goal`"));
    assert!(refused(&format!("{text}trailing\n")).contains("expected `goal`"));
    assert!(refused(&"x".repeat(600 * 1024)).contains("too large"));
}

#[test]
fn a_record_that_does_not_add_up_is_refused_with_its_line() {
    let text = good();
    let cases: [(&str, &str, &str); 14] = [
        // Counts that are not the log's.
        (
            "spent\t1\t1\t0\t0\t300",
            "spent\t2\t1\t0\t0\t300",
            "step counts",
        ),
        (
            "spent\t1\t1\t0\t0\t300",
            "spent\t1\t0\t1\t0\t300",
            "step counts",
        ),
        (
            "spent\t1\t1\t0\t0\t300",
            "spent\t1\t1\t0\t0\t299",
            "fewer tokens",
        ),
        // A budget out of range, or already passed by the log.
        (
            "budget\t6\t20000\t36000000",
            "budget\t0\t20000\t36000000",
            "budget",
        ),
        (
            "budget\t6\t20000\t36000000",
            "budget\t65\t20000\t36000000",
            "budget",
        ),
        (
            "budget\t6\t20000\t36000000",
            "budget\t6\t0\t36000000",
            "budget",
        ),
        // A scope that is not allowed.
        ("folders\treports", "folders\t../etc", "scope"),
        ("folders\treports", "folders\t/home/user", "scope"),
        ("folders\treports", "folders\treports\treports", "scope"),
        (
            "rows\tfs_list\tfs_trash\tnotes_*",
            "rows\tfs_list\tFS",
            "scope",
        ),
        // States that need lines they do not have.
        ("goal\t3\tactive", "goal\t3\twaiting", "waiting step"),
        (
            "goal\t2\twaiting",
            "goal\t2\tactive",
            "state does not match",
        ),
        ("ended\t-", "ended\t5", "end"),
        ("goal\t1\tdone", "goal\t0\tdone", "ascending"),
    ];
    for (from, to, expected) in cases {
        assert!(text.contains(from), "{from:?} is not in the good file");
        let why = refused(&text.replacen(from, to, 1));
        assert!(why.starts_with("line "), "{why}");
        assert!(why.contains(expected), "{expected:?} is not in {why:?}");
    }
}

#[test]
fn words_and_numbers_the_log_could_not_hold_are_refused() {
    let text = good();
    for (from, to) in [
        (
            "text\ttidy the reports folder",
            "text\ttidy the reports folder\u{e9}",
        ),
        (
            "text\ttidy the reports folder",
            "text\t tidy the reports folder",
        ),
        ("text\ttidy the reports folder", "text\t"),
        ("text\ttidy the reports folder", "text\ttidy\x7f"),
        ("by\t29800000", "by\t029800000"),
        ("by\t29800000", "by\t+29800000"),
        ("by\t29800000", "by\t18446744073709551616"),
        ("away\t1", "away\t2"),
        ("goal\t1\tdone", "goal\t1\tsleeping"),
        ("budget\t6\t20000\t36000000", "budget\t6\t20000"),
        (
            "budget\t6\t20000\t36000000",
            "budget\t6\t20000\t36000000\t1",
        ),
    ] {
        assert!(text.contains(from), "{from:?}");
        refused(&text.replacen(from, to, 1));
    }
}

#[test]
fn a_waiting_or_running_step_that_is_not_the_last_is_refused() {
    let text = good();
    // The waiting goal's ask names a row its last step does not.
    let waiting = text
        .lines()
        .find(|line| line.starts_with("waiting\t"))
        .unwrap();
    let other = waiting.replace("reports/old.txt", "reports/other.txt");
    assert!(refused(&text.replacen(waiting, &other, 1)).contains("waits on a step"));
    // The running goal's flight points at a step that is not running.
    assert!(refused(&text.replacen("flight\t1", "flight\t2", 1)).contains("running"));
    // A flight with no running step.
    let step = text
        .lines()
        .find(|line| line.starts_with("step\t1\t") && line.contains("\trunning\t"))
        .unwrap();
    let ran = step.replacen("\trunning\t", "\tran\t", 1);
    refused(&text.replacen(step, &ran, 1));
}

#[test]
fn more_goals_than_are_kept_or_active_are_refused() {
    let mut goals = Goals::new();
    for _ in 0..MAX_GOALS {
        goals.set(spec(), T0).unwrap();
    }
    let text = goals.to_text();
    assert!(Goals::parse(&text).is_ok());
    let extra = text.replacen("ids\t17", "ids\t18", 1)
        + &text[text.find("goal\t1\t").unwrap()..text.find("goal\t2\t").unwrap()].replacen(
            "goal\t1\t",
            "goal\t17\t",
            1,
        );
    assert!(refused(&extra).contains("too many goals"));

    // Five active goals: the storm rule does not hold.
    let mut five = Goals::new();
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(five.set(spec(), T0).unwrap());
    }
    for id in &ids[..4] {
        five.approve(*id, T0).unwrap();
    }
    let text = five.to_text();
    let fifth = text.replacen("goal\t5\tproposed", "goal\t5\tactive", 1);
    let fifth = fifth.replacen("started\t-", "started\t1000000", 1);
    assert!(refused(&fifth).contains("active"));
}

/// Every way of damaging one line of a good file: the result is refused, or it is
/// itself a good file that writes back as it is. Never a file that is accepted and
/// changed on the way through.
#[test]
fn damaging_any_line_is_refused_or_reads_back_unchanged() {
    let text = good();
    let lines: Vec<&str> = text.lines().collect();
    let join = |lines: &[String]| format!("{}\n", lines.join("\n"));
    let base: Vec<String> = lines.iter().map(|line| (*line).to_string()).collect();
    let mut accepted = 0;
    let mut tried = 0;
    let mut check = |mutated: String| {
        tried += 1;
        if let Ok(goals) = Goals::parse(&mutated) {
            accepted += 1;
            assert_eq!(goals.to_text(), mutated, "accepted but changed");
        }
    };
    for at in 0..base.len() {
        // The line gone, twice over, or swapped with the next one.
        let mut gone = base.clone();
        gone.remove(at);
        check(join(&gone));
        let mut twice = base.clone();
        twice.insert(at, base[at].clone());
        check(join(&twice));
        if at + 1 < base.len() {
            let mut swapped = base.clone();
            swapped.swap(at, at + 1);
            check(join(&swapped));
        }
        // A field damaged: dropped, added, emptied, or made garbage.
        let fields: Vec<&str> = base[at].split('\t').collect();
        for field in 1..fields.len() {
            for new in ["", "x", "-", "0", "99999999999", "a b", "\u{e9}", "../x"] {
                let mut damaged = base.clone();
                let mut parts: Vec<String> = fields.iter().map(|f| (*f).to_string()).collect();
                parts[field] = new.to_string();
                damaged[at] = parts.join("\t");
                check(join(&damaged));
            }
            let mut dropped = base.clone();
            let mut parts: Vec<String> = fields.iter().map(|f| (*f).to_string()).collect();
            parts.remove(field);
            dropped[at] = parts.join("\t");
            check(join(&dropped));
        }
        let mut added = base.clone();
        added[at] = format!("{}\textra", base[at]);
        check(join(&added));
    }
    assert!(tried > 500, "only {tried} tried");
    // A few harmless ones are accepted: a text, a note or a path swapped for other
    // good words. Nearly all are not.
    assert!(accepted * 4 < tried, "{accepted} of {tried} accepted");
}
