use super::*;
use infer::json::Constraint;
use render::Format;

fn args(text: &str) -> Vec<String> {
    text.split(' ').map(String::from).collect()
}

#[test]
fn arms_and_seeds_parse() {
    let o = parse(&args(
        "--local q --arm dsl-1,hermes-1@80,dsl-1:no-think,m8-a0 --seed 42",
    ))
    .unwrap();
    assert_eq!(
        o.arms.iter().map(Arm::label).collect::<Vec<_>>(),
        ["dsl-1@40", "hermes-1@80", "dsl-1:no-think@40", "m8-a0"]
    );
    assert_eq!(o.seeds, [42]);
    assert!(parse(&args("--arm dsl-1")).is_err());
    assert!(parse(&args("--local q --arm dsl-2")).is_err());
    assert!(parse(&args("--local q --arm dsl-1@60")).is_err());
    assert_eq!(parse(&args("--local q")).unwrap().seeds, SEEDS);
    let o = parse(&args(
        "--local q --arm dsl-1+rules,dsl-1+m8script@80 --only-m8",
    ))
    .unwrap();
    let labels: Vec<String> = o.arms.iter().map(Arm::label).collect();
    assert_eq!(labels, ["dsl-1+rules@40", "dsl-1+m8script@80"]);
    assert!(o.only_m8 && o.arms[0].rules && o.arms[1].m8script);
}

#[test]
fn the_layout_is_pack_mode_history_then_state_and_request() {
    let task = TASKS.iter().find(|t| !t.history.is_empty()).unwrap();
    let turns = task_turns("PACK", task, false);
    assert_eq!(turns[0], (Role::System, "PACK".into()));
    assert_eq!(turns[1].1, render::MODE_ACT);
    assert_eq!(turns.len(), 2 + 3 * task.history.len() + 1);
    assert_eq!(
        user_text("focused=x", "stop this", true),
        "State: focused=x\nRequest: stop this\n/no_think"
    );
    assert_eq!(chosen(Some(1)).len(), Cat::ALL.len());
}

/// The whole loop on the generated local-script model: every task decodes
/// to a call the grammar closes, the M8 tasks go through the shell, and the
/// answers are the same at a second run (seeded) and on one worker.
#[test]
fn the_harness_runs_end_to_end_on_the_fixture_model() {
    // A vocabulary trained on the pack too, so it fits the fixture's context.
    let text = [
        crate::local_script::prompts(),
        render::pack(Format::Dsl1, &registry::rows(40)),
        render::MODE_ACT.to_string(),
        llmproto::pack::render(llmproto::pack::Format::Dsl1),
    ]
    .join("\n");
    let bytes = infer::fixture::local_script(&text);
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let loaded = load(bytes, "fixture").unwrap();
    let rows = registry::rows(40);
    let grammar = grammar::tagged(&rows);
    let pack = render::pack(Format::Dsl1, &rows);
    let task = &TASKS[0];
    let turns = task_turns(&pack, task, false);
    let first = decode(&mut loaded.engine.borrow_mut(), &grammar, &turns, 42).unwrap();
    let again = decode(&mut loaded.engine.borrow_mut(), &grammar, &turns, 42).unwrap();
    assert_eq!(first.text, again.text);
    assert!(again.cached > 0);
    if first.ending == Ending::EndToken {
        let call = first.call.expect("a closed answer parses");
        assert!(
            grammar.is_done(
                &grammar
                    .advance(grammar.start(), first.text.as_bytes())
                    .unwrap()
            )
        );
        assert!(call["arguments"].is_object());
    }
    let dir = std::env::temp_dir().join(format!("actbench-test-{}", std::process::id()));
    let options = parse(&args("--local fixture --limit 1 --seed 42")).unwrap();
    run_arm(&loaded, Arm::parse("dsl-1").unwrap(), 42, &options, &dir).unwrap();
    run_arm(&loaded, Arm::parse("m8-a0").unwrap(), 42, &options, &dir).unwrap();
    // What ships: the M8 task through the shell's one door, on the OS pack.
    let (_, rows) = run_arm(&loaded, Arm::parse("act").unwrap(), 42, &options, &dir).unwrap();
    assert!(
        rows.contains("### act seed 42") && rows.contains("OS pack_id `"),
        "{rows}"
    );
    assert!(rows.contains("| exact single calls (>= 57/60) |"), "{rows}");
    let files = fs::read_dir(dir.join("actbench")).unwrap().count();
    assert_eq!(files, 3);
    write_summary(&rows, &dir).unwrap();
    assert!(dir.join("actbench").join("summary.md").exists());
    let _ = fs::remove_dir_all(&dir);
}
