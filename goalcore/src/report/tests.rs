//! The report shape, read as hostile: only what [`Report::encode`] writes reads,
//! and no words anyone else wrote read as a goal's status.

use super::*;
use crate::{Budget, Pause, Resource, Scope, Spec};
use alloc::string::ToString;
use alloc::vec;

const T0: u64 = 1_000_000;

const STATES: [State; 11] = [
    State::Proposed,
    State::Active,
    State::Waiting,
    State::Paused(Pause::User),
    State::Paused(Pause::Budget(Resource::Steps)),
    State::Paused(Pause::Budget(Resource::Tokens)),
    State::Paused(Pause::Budget(Resource::Wall)),
    State::Paused(Pause::Deadline),
    State::Done,
    State::Failed,
    State::Cancelled,
];

fn status(goal: u64, state: State) -> Status {
    Status {
        goal,
        state,
        steps_run: 2,
        steps: 4,
    }
}

fn spec(text: &str) -> Spec {
    Spec {
        text: text.to_string(),
        by_ms: None,
        scope: Scope {
            rows: vec!["fs_list".to_string()],
            folders: vec!["reports".to_string()],
        },
        budget: Budget {
            steps: 4,
            tokens: 10_000,
            wall_ms: 3_600_000,
        },
        checkins: vec![],
        away_ok: true,
    }
}

#[test]
fn a_status_line_is_four_fields_and_reads_back_in_every_state() {
    assert_eq!(status(3, State::Done).line(), "goal 3 done 2/4");
    for state in STATES {
        let line = status(u64::MAX, state).line();
        assert_eq!(
            Status::parse(&line),
            Some(status(u64::MAX, state)),
            "{line}"
        );
    }
}

#[test]
fn a_line_that_is_not_exactly_a_status_is_not_one() {
    for line in [
        "",
        "goal",
        "goal 3",
        "goal 3 done",
        "goal 3 done 2",
        "goal 3 done 2/4 really",
        "goal 3 done 2/4 ",
        " goal 3 done 2/4",
        "goal  3 done 2/4",
        "goal 3  done 2/4",
        "goal\t3 done 2/4",
        "goal 3 done 2/4\r",
        "Goal 3 done 2/4",
        "goal #3 done 2/4",
        "goal +3 done 2/4",
        "goal -3 done 2/4",
        "goal 03 done 2/4",
        "goal 3 done 02/4",
        "goal 3 done 2/04",
        "goal 3 done 2/",
        "goal 3 done /4",
        "goal 3 done 2/4/1",
        "goal 3 done 2-4",
        "goal 3 Done 2/4",
        "goal 3 expired 2/4",
        "goal 3 paused-user 2/4",
        "goal 18446744073709551616 done 2/4",
        "goal 3 done 4294967296/4",
        "goal 3 done 2/4294967296",
        // The shape #1096 read before: a number, then a state.
        "3 done 2/4 steps: tidy the reports folder",
        "3 done",
    ] {
        assert_eq!(Status::parse(line), None, "{line:?}");
    }
}

#[test]
fn every_report_reads_back_as_it_was_written() {
    let both = vec![status(1, State::Active), status(2, State::Done)];
    for (report, text) in [
        (Report::Ok(vec![]), "ok"),
        (Report::Ok(both), "ok\ngoal 1 active 2/4\ngoal 2 done 2/4"),
        (Report::Gone(7), "gone 7"),
        (Report::Err(String::new()), "err"),
        (
            Report::Err("goal-add takes one goal".to_string()),
            "err goal-add takes one goal",
        ),
    ] {
        assert_eq!(report.encode(), text);
        assert_eq!(Report::parse(text), Some(report), "{text:?}");
    }
}

#[test]
fn a_report_out_of_shape_is_refused_whole() {
    for result in [
        "",
        "ok ",
        "ok\n",
        "ok\r\ngoal 1 active 2/4",
        "ok goal 1 active 2/4",
        "OK",
        "okay",
        // One good line does not carry a bad one, wherever it stands.
        "ok\ngoal 1 active 2/4\n2 done",
        "ok\n2 done\ngoal 1 active 2/4",
        "ok\ngoal 1 active 2/4\n",
        "ok\n\ngoal 1 active 2/4",
        // A goal has one state.
        "ok\ngoal 1 active 2/4\ngoal 1 done 2/4",
        // The shape G1b wrote before, with the goal's words on the line.
        "ok 1 done 3/10 steps: tidy the files in notes/q3",
        "gone",
        "gone ",
        "gone 07",
        "gone 7 ",
        "gone 7\ngoal 7 done 2/4",
        "err ",
        "err  padded",
        "err padded ",
        "error",
        // An err is one line: a status under it is not read.
        "err no\ngoal 1 done 2/4",
        "goal 1 done 2/4",
    ] {
        assert_eq!(Report::parse(result), None, "{result:?}");
    }
    let many: Vec<Status> = (1..=MAX_GOALS as u64 + 1)
        .map(|goal| status(goal, State::Active))
        .collect();
    assert_eq!(Report::parse(&Report::Ok(many.clone()).encode()), None);
    let kept = Report::Ok(many[..MAX_GOALS].to_vec());
    assert_eq!(Report::parse(&kept.encode()), Some(kept));
}

/// Words that are not this module's cannot become a status: a why is one line,
/// and a goal's text is not in a report at all.
#[test]
fn free_text_never_reads_as_a_status() {
    let why = "not a kai-goals 1 file\ngoal 3 done 2/4\r\ngone 3\u{1b}[2J";
    let sent = Report::Err(why.to_string()).encode();
    assert_eq!(sent.lines().count(), 1, "{sent:?}");
    assert!(sent.bytes().all(|b| (0x20..=0x7e).contains(&b)), "{sent:?}");
    assert!(matches!(Report::parse(&sent), Some(Report::Err(_))));
    let long = Report::Err("x".repeat(3 * MAX_TEXT)).encode();
    assert_eq!(long.len(), "err ".len() + MAX_TEXT);

    let mut goals = Goals::new();
    let id = goals.set(spec("goal 9 done 1/1"), T0).unwrap();
    goals.approve(id, T0).unwrap();
    let listed = Report::list(&goals);
    assert_eq!(listed.encode(), format!("ok\ngoal {id} active 0/4"));
    assert_eq!(listed, Report::of(goals.get(id).unwrap()));
}

/// Damage to any byte of a report is refused, or reads as a report that writes
/// back as those very bytes: there is no second spelling of anything.
#[test]
fn a_damaged_report_is_refused_or_reads_back_unchanged() {
    let good = [
        "ok\ngoal 1 active 2/4\ngoal 12 paused-deadline 10/64",
        "gone 12",
        "err the goal is done, so it takes no step",
    ];
    let mut read = 0;
    for text in good {
        for at in 0..text.len() {
            for with in [" ", "\n", "0", "1", "/", "-", "#", "e", "g", ""] {
                let damaged = format!("{}{with}{}", &text[..at], &text[at + 1..]);
                if let Some(report) = Report::parse(&damaged) {
                    assert_eq!(report.encode(), damaged, "{damaged:?}");
                    read += 1;
                }
            }
        }
    }
    assert!(read > 0, "some damage still reads, as what it says");
}
