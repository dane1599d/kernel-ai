//! The goal's text form.

use super::*;

// ---- the text form

fn busy_goals() -> Goals {
    let mut goals = Goals::new();
    let done_id = goals.set(spec(), T0).unwrap();
    let waiting_id = goals.set(spec(), T0).unwrap();
    let running_id = goals.set(spec(), T0).unwrap();
    let paused_id = goals.set(spec(), T0).unwrap();
    let asking_id = goals.set(spec(), T0).unwrap();
    let _proposed = goals.set(spec(), T0).unwrap();
    for id in [done_id, waiting_id, running_id, paused_id] {
        goals.approve(id, T0 + 1).unwrap();
    }
    let goal = goals.get_mut(done_id).unwrap();
    goal.plan(2, "all done");
    ran(
        goal,
        T0 + 2,
        step("fs_create", &["reports/a b.txt"]),
        5,
        300,
    );
    goal.charge(25);
    goal.complete(T0 + 3, "wrote it").unwrap();
    goal.record_met(true).unwrap();
    let goal = goals.get_mut(waiting_id).unwrap();
    goal.plan(3, "trash the old one");
    ran(goal, T0 + 2, run_step("fs_list"), 6, 10);
    let p = step("fs_trash", &["reports/old.txt"]);
    goal.propose(T0 + 4, p, Presence::Away, &DOOR).unwrap();
    let goal = goals.get_mut(running_id).unwrap();
    goal.propose(T0 + 5, run_step("notes_add"), Presence::Here, &DOOR)
        .unwrap();
    let goal = goals.get_mut(paused_id).unwrap();
    let denied = step("service_restart", &[]);
    goal.propose(T0 + 6, denied, Presence::Here, &DOOR).unwrap();
    goal.deny().unwrap();
    goal.pause().unwrap();
    goals.approve(asking_id, T0 + 1).unwrap();
    let goal = goals.get_mut(asking_id).unwrap();
    goal.plan(2, "after the answer");
    goal.clarify("Which folder holds the Q1 notes?").unwrap();
    goals.tick(T0 + 2 * HOUR);
    goals
}

#[test]
fn goals_read_back_as_they_were_written_byte_for_byte() {
    let goals = busy_goals();
    let text = goals.to_text();
    let back = Goals::parse(&text).unwrap();
    assert_eq!(back, goals);
    assert_eq!(back.to_text(), text);
    assert!(text.starts_with("kai-goals 1\nids\t7\ngoal\t1\tdone\n"));
    // A question that waits is a `waiting` line with no step, and a goal the
    // runner checked has its `met`.
    assert!(text.contains("\nwaiting\tquestion\tWhich folder holds the Q1 notes?\n"));
    assert!(text.contains("\nmet\t1\n"));
}

#[test]
fn an_empty_file_of_goals_round_trips() {
    let text = Goals::new().to_text();
    assert_eq!(text, "kai-goals 1\nids\t1\n");
    assert_eq!(Goals::parse(&text).unwrap(), Goals::new());
}

#[test]
fn the_text_form_has_one_line_a_field_in_a_fixed_order() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    goals.approve(id, T0).unwrap();
    let text = goals.to_text();
    let expected = "kai-goals 1
ids\t2
goal\t1\tactive
text\tdraft the quarterly report
by\t29800000
away\t1
made\t1000000
started\t1000000
ended\t-
met\t-
note\t
rows\tfs_list\tfs_read\tfs_create\tfs_trash\tnotes_*
folders\treports
budget\t4\t10000\t36000000
checkins\t8200000\t15400000
plan\t0\t
spent\t0\t0\t0\t0\t0\t0
posted\t0\t0\t0
end
";
    assert_eq!(text, expected);
}
