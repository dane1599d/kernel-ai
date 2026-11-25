use super::*;
use infer::json::Constraint;
use llmproto::pack::estimate_tokens;

fn args(text: &str) -> Vec<String> {
    text.split_whitespace().map(String::from).collect()
}

/// The call `task` expects, as the model would write it in full: its
/// arguments in the row's order, which the grammar holds it to.
fn expected(task: &Task) -> Option<String> {
    let arguments: Vec<String> = task
        .args
        .iter()
        .map(|(arg, want)| {
            let value = match want {
                Want::Text(text) => Value::String((*text).into()),
                Want::Int(n) => Value::from(*n),
            };
            format!("{}:{value}", Value::String((*arg).into()))
        })
        .collect();
    Some(format!(
        r#"{{"name":"{}","arguments":{{{}}}}}"#,
        task.row?,
        arguments.join(",")
    ))
}

fn call(text: &str) -> Call {
    llmproto::act::parse(text).unwrap()
}

#[test]
fn the_eight_stub_apps_load_and_their_index_lines_fit_the_apps_budget() {
    let groups = stubs::groups().unwrap();
    assert_eq!(groups.len(), 8);
    let rows: usize = groups.iter().map(|g| g.rows.len()).sum();
    assert_eq!(rows, 22);
    let (turn, left_out) = apps::turn(&groups);
    assert!(left_out.is_empty(), "{left_out:?}");
    assert!(estimate_tokens(&turn) <= apps::APPS_BUDGET, "{turn}");
    let (flat, left_out) = apps::flat_turn(&groups);
    assert!(left_out.is_empty());
    for group in &groups {
        assert!(turn.contains(&format!("\n{}:", group.app)), "{turn}");
        assert!(flat.contains(&format!("## {}\n", group.app)), "{flat}");
    }
}

#[test]
fn there_are_120_tasks_and_each_expects_a_whole_call_of_one_stub_row() {
    let count = |cat: Cat| TASKS.iter().filter(|t| t.cat == cat).count();
    assert_eq!(
        (
            count(Cat::Single),
            count(Cat::Confusable),
            count(Cat::NoApp)
        ),
        (80, 20, 20)
    );
    for stub in &stubs::STUBS {
        let prefix = format!("{}_", stub.app);
        let single = TASKS
            .iter()
            .filter(|t| t.cat == Cat::Single && t.row.is_some_and(|r| r.starts_with(&prefix)))
            .count();
        assert_eq!(single, 10, "{}", stub.app);
    }
    for task in &TASKS {
        let Some(name) = task.row else {
            assert!(task.args.is_empty(), "{}", task.line);
            continue;
        };
        let (_, row) = stubs::find(name).unwrap_or_else(|| panic!("{}: no row {name}", task.line));
        let named: Vec<&str> = task.args.iter().map(|(a, _)| *a).collect();
        let declared: Vec<&str> = row.args.iter().map(|(a, _)| *a).collect();
        assert_eq!(named, declared, "{}", task.line);
        for (arg, want) in task.args {
            if let Want::Text(text) = want {
                // Expected text is matched in lower case.
                assert_eq!(*text, text.to_lowercase(), "{}: {arg}", task.line);
            }
        }
    }
}

#[test]
fn every_expected_call_decodes_flat_in_one_round_and_layered_by_name() {
    let groups = stubs::groups().unwrap();
    let flat = Arm::Flat.grammar(&groups).unwrap();
    let layered = Arm::Layered.grammar(&groups).unwrap();
    for task in &TASKS {
        let Some(text) = expected(task) else {
            continue;
        };
        let g = flat.first();
        let state = g
            .advance(g.start(), text.as_bytes())
            .unwrap_or_else(|e| panic!("{text}: {e:?}"));
        assert!(g.is_done(&state), "{text}");
        assert!(passed(task, &groups, &call(&text)), "{text}");
        assert_eq!(right_group(task, &groups, Some(&call(&text))), Some(true));
        let named = format!(r#"{{"name":"{}","arguments":{{}}}}"#, task.row.unwrap());
        let g = layered.first();
        let state = g
            .advance(g.start(), named.as_bytes())
            .unwrap_or_else(|e| panic!("{named}: {e:?}"));
        assert!(g.is_done(&state), "{named}");
    }
}

#[test]
fn a_call_passes_on_its_row_and_every_expected_argument() {
    let groups = stubs::groups().unwrap();
    let task = TASKS
        .iter()
        .find(|t| t.line.starts_with("add milk"))
        .unwrap();
    let pass = call(r#"{"name":"shopping_add","arguments":{"item":"Milk, 2 litres"}}"#);
    assert!(passed(task, &groups, &pass));
    let other_item = call(r#"{"name":"shopping_add","arguments":{"item":"bread"}}"#);
    assert!(!passed(task, &groups, &other_item));
    assert_eq!(right_group(task, &groups, Some(&other_item)), Some(true));
    let other_app = call(r#"{"name":"todo_add","arguments":{"text":"milk"}}"#);
    assert!(!passed(task, &groups, &other_app));
    assert_eq!(right_group(task, &groups, Some(&other_app)), Some(false));
    assert_eq!(right_group(task, &groups, None), Some(false));
    let timer = TASKS
        .iter()
        .find(|t| t.line == "start a 5 minute timer")
        .unwrap();
    let seconds = |n: u64| {
        call(&format!(
            r#"{{"name":"timer_start","arguments":{{"seconds":{n}}}}}"#
        ))
    };
    assert!(passed(timer, &groups, &seconds(300)));
    assert!(!passed(timer, &groups, &seconds(5)));
}

#[test]
fn a_task_for_no_app_passes_on_any_call_but_an_app_row() {
    let groups = stubs::groups().unwrap();
    let task = TASKS.iter().find(|t| t.cat == Cat::NoApp).unwrap();
    assert!(passed(
        task,
        &groups,
        &call(r#"{"name":"script","arguments":{}}"#)
    ));
    assert!(passed(
        task,
        &groups,
        &call(r#"{"name":"say","arguments":{"text":"Hi."}}"#)
    ));
    assert!(!passed(
        task,
        &groups,
        &call(r#"{"name":"todo_list","arguments":{}}"#)
    ));
    assert_eq!(right_group(task, &groups, None), None);
}

#[test]
fn options_name_the_arms_and_the_gate_needs_both() {
    let options = parse(&args("--local m --arm flat --limit 2")).unwrap();
    assert_eq!(options.arms, [Arm::Flat]);
    assert_eq!(options.limit, Some(2));
    assert_eq!(
        parse(&args("--local m")).unwrap().arms,
        [Arm::Layered, Arm::Flat]
    );
    assert!(parse(&args("--local m --arm flat --gate")).is_err());
    assert!(parse(&args("--local m --arm wide")).is_err());
    assert!(parse(&args("--arm flat")).is_err());
    assert_eq!(chosen(Some(2)).len(), 6);
    assert_eq!(chosen(None).len(), 120);
}

#[test]
fn the_gate_allows_two_points_and_needs_95_percent_right() {
    let tally = |passed: usize, right: usize| Tally {
        cats: [(passed, 100), (0, 0), (0, 0)],
        right: (right, 100),
        ..Tally::default()
    };
    assert!(verdict(&tally(88, 95), &tally(90, 100)).0);
    assert!(!verdict(&tally(87, 95), &tally(90, 100)).0);
    assert!(!verdict(&tally(95, 94), &tally(90, 100)).0);
    let (_, text) = verdict(&tally(87, 95), &tally(90, 100));
    assert!(
        text.contains("| -3.0 points |") && text.contains("FAIL"),
        "{text}"
    );
}

#[test]
fn the_gate_fails_when_an_arm_did_not_answer_every_task() {
    let tally = |passed: usize, errors: usize, context_full: usize| Tally {
        cats: [(passed, 100), (0, 0), (0, 0)],
        right: (100, 100),
        errors,
        context_full,
        ..Tally::default()
    };
    // A flat arm whose every decode erred scores 0: nothing was compared.
    let (passes, text) = verdict(&tally(90, 0, 0), &tally(0, 100, 0));
    assert!(!passes);
    assert!(
        text.contains("| flat: tasks with no call | 100 (0 context full) | 0 | FAIL |"),
        "{text}"
    );
    assert!(!verdict(&tally(90, 0, 0), &tally(90, 1, 1)).0);
    assert!(!verdict(&tally(90, 1, 0), &tally(90, 0, 0)).0);
    assert!(!verdict(&tally(90, 0, 1), &tally(90, 0, 0)).0);
    assert!(verdict(&tally(90, 0, 0), &tally(90, 0, 0)).0);
}

#[test]
fn both_arms_run_end_to_end_on_the_fixture_model() {
    let groups = stubs::groups().unwrap();
    let text = [
        crate::local_script::prompts(),
        llmproto::pack::render(llmproto::pack::Format::Dsl1),
        apps::flat_turn(&groups).0,
    ]
    .join("\n");
    let bytes: &'static [u8] = Box::leak(infer::fixture::local_script(&text).into_boxed_slice());
    let dir = std::env::temp_dir().join(format!("groups-test-{}", std::process::id()));
    let tasks = chosen(Some(1));
    for arm in [Arm::Layered, Arm::Flat] {
        let loaded = actbench::load_at(bytes, "fixture", arm.context()).unwrap();
        let tally = run_arm(&loaded, arm, &groups, &tasks, &dir).unwrap();
        assert_eq!(tally.passed().1, 3);
        assert_eq!(tally.right.1, 1);
        assert!(tally.prompt_positions > 0);
        assert_eq!(tally.context_full, 0, "{}", arm.name());
        let text = rows(arm, &loaded.line, &tally);
        assert!(text.contains(&format!("### {} (", arm.name())), "{text}");
        assert!(text.contains(&loaded.line), "{text}");
        let written = fs::read_to_string(dir.join("groups").join(format!("{}.jsonl", arm.name())));
        let written = written.unwrap();
        assert_eq!(written.lines().count(), 3);
        // Each answer is a call, or the fixture's reply cut off at the token
        // limit; never an engine error.
        let cut_off = llmproto::act::ActError::CutOff.describe();
        let mut cut = 0;
        for line in written.lines() {
            let record: Value = serde_json::from_str(line).unwrap();
            let got = record["got"].as_str().unwrap();
            if got == cut_off {
                cut += 1;
            } else {
                assert!(llmproto::act::parse(got).is_ok(), "{}: {got}", arm.name());
            }
        }
        assert_eq!(tally.errors, cut, "{}", arm.name());
    }
    write_summary("## groups\n", &dir).unwrap();
    assert!(dir.join("groups").join("summary.md").exists());
    let _ = fs::remove_dir_all(&dir);
}
