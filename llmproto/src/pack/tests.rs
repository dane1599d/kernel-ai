extern crate std;

use super::*;

/// The golden `dsl-1` pack. A row change shows here as a diff; to accept one,
/// run with `KAI_BLESS=1` and review the file.
const GOLDEN: &str = include_str!("dsl-1.txt");

#[test]
fn dsl_1_renders_its_golden() {
    let pack = render(Format::Dsl1);
    if std::env::var_os("KAI_BLESS").is_some() {
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/pack/dsl-1.txt"),
            &pack,
        )
        .unwrap();
        return;
    }
    assert_eq!(pack, GOLDEN.replace("\r\n", "\n"));
}

#[test]
fn m8_is_todays_prompt_byte_for_byte() {
    assert_eq!(render(Format::M8), crate::script::system_prompt());
}

#[test]
fn the_pack_and_mode_fit_the_budget_by_estimate() {
    let pack = render(Format::Dsl1);
    let estimate = estimate_tokens(&pack) + estimate_tokens(MODE_ACT);
    std::println!("pack and MODE: {estimate} tokens, estimated");
    assert!(estimate <= PACK_BUDGET, "{estimate} tokens, estimated");
}

#[test]
fn every_line_is_printable_ascii_of_at_most_120_bytes() {
    for line in render(Format::Dsl1).lines() {
        assert!(line.len() <= 120, "{line}");
        assert!(line.bytes().all(|b| (b' '..=b'~').contains(&b)), "{line}");
    }
}

#[test]
fn every_script_function_and_command_has_a_row_and_no_helper_does() {
    let pack = render(Format::Dsl1);
    for row in agentabi::BOUND.iter().chain(&agentabi::COMMANDS) {
        assert!(super::shows(&pack, row.name), "{}", row.name);
    }
    for row in agentabi::HELPERS {
        assert!(
            !pack.contains(&std::format!("\n{}(", row.name)),
            "{}",
            row.name
        );
    }
}

#[test]
fn marks_follow_the_derived_tier() {
    let line = |row| dsl1::line(row);
    assert_eq!(
        line(&agentabi::SET_POLICY).split("  //").next(),
        Some("set_policy(name: Policy) [undo]")
    );
    assert!(line(&agentabi::CPUS).starts_with("cpus() -> int  // "));
    assert!(line(&agentabi::TUNE).contains(" [ask]  // "));
    assert!(line(&agentabi::COMPONENT_REVERT).contains("back: int 1..6?"));
}

/// ADR 0010: the pack must not teach the model to confirm a run-tier call first.
#[test]
fn the_pack_never_asks_the_model_to_confirm() {
    for format in Format::ALL {
        let text = render(format).to_lowercase() + MODE_ACT;
        assert!(!text.contains("confirm"), "{}", format.name());
    }
}

#[test]
fn pack_ids_differ_by_format_tokenizer_and_bytes() {
    let id = pack_id(Format::Dsl1, "t", "p");
    assert_ne!(id, pack_id(Format::M8, "t", "p"));
    assert_ne!(id, pack_id(Format::Dsl1, "u", "p"));
    assert_ne!(id, pack_id(Format::Dsl1, "t", "q"));
}

#[test]
fn every_brief_doc_names_a_row_and_is_short() {
    for (name, doc) in dsl1::BRIEF {
        assert!(agentabi::find(&agentabi::ROWS, name).is_some(), "{name}");
        assert!(doc.len() <= 60, "{name}");
    }
}

/// A family's rows share one line, marks in order, and a row of another
/// signature keeps its own line.
#[test]
fn a_family_shares_one_line() {
    use agentabi::Action;
    let start = Action {
        name: "service_start",
        ..agentabi::SET_POLICY
    };
    let stop = Action {
        name: "service_stop",
        args: agentabi::SET_POLICY.args,
        ..agentabi::TUNE
    };
    let other = Action {
        name: "service_list",
        ..agentabi::CPUS
    };
    let lines = dsl1::row_lines(&[&agentabi::CPUS, &start, &other, &stop]);
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(
        lines[1],
        std::format!(
            "service_start|stop(name: Policy) [undo|ask]  // {}",
            dsl1::FAMILIES[0].1
        )
    );
    assert!(lines[2].starts_with("service_list() -> int  // "));
    // One member alone is a row like any other.
    assert_eq!(dsl1::row_lines(&[&start]), [dsl1::line(&start)]);
}

/// A script call carries no code: the pack teaches only the call the grammar
/// allows.
#[test]
fn the_pack_teaches_script_with_no_arguments() {
    let pack = render(Format::Dsl1);
    assert!(pack.contains("\nscript()  // "));
    assert!(!pack.contains(r#""code":"#));
}

/// ADR 0010 §6: an index-only row is named once on its group's line and has no
/// line of its own; its group's text gives the full lines for the second round.
#[test]
fn index_rows_are_named_once_and_shown_in_full_on_demand() {
    let pack = render(Format::Dsl1);
    for (group, names) in INDEX {
        let text = group_text(group);
        assert!(text.starts_with("## "), "{text}");
        // The pack without its index lines shows none of the group's rows.
        let full = pack.split(INDEX_HEADING).next().unwrap_or("");
        for name in *names {
            assert!(agentabi::find(&agentabi::ROWS, name).is_some(), "{name}");
            assert!(shows(&pack, name), "{name}");
            assert!(!shows(full, name), "{name}");
            assert!(shows(&text, name), "{text}");
            assert_eq!(indexed(name).map(|(g, _)| g), Some(*group));
        }
    }
    assert!(group_text("component").contains("component_revert("));
    // Every group has its gloss, short, and on its line before the names.
    for (group, _) in INDEX {
        let gloss = super::gloss(group);
        assert!(!gloss.is_empty(), "{group}");
        assert!(
            estimate_tokens(gloss) <= super::GLOSS_TOKENS,
            "{group}: {gloss}"
        );
        assert!(!gloss.contains(": "), "{group}: {gloss}");
        assert!(
            pack.contains(&std::format!("\n{group} ({gloss}): ")),
            "{group}"
        );
    }
    assert!(group_text("files").starts_with("## files (paths under /home/user)\n"));
    assert_eq!(indexed("set_policy"), None);
    assert_eq!(indexed("tune"), None);
    // An index line is not a row's own line: no other name is shown by it.
    assert!(!shows(&pack, "component_list"));
}

/// An app row for the tests: an OS row's shape under an app's name.
fn app_row(name: &'static str, like: &str) -> &'static Action {
    let like = agentabi::find(&agentabi::ROWS, like).unwrap();
    std::boxed::Box::leak(std::boxed::Box::new(Action { name, ..*like }))
}

fn notes() -> apps::AppGroup {
    apps::AppGroup {
        app: "notes".into(),
        rows: std::vec![
            app_row("notes_add", "fs_create"),
            app_row("notes_count", "cpus")
        ],
    }
}

#[test]
fn the_apps_turn_names_each_apps_rows_on_one_line() {
    let (turn, left_out) = apps::turn(&[notes()]);
    assert_eq!(
        turn,
        std::format!("{}\nnotes: notes_add notes_count", apps::APPS_HEADING)
    );
    assert!(left_out.is_empty());
    // Its full lines are the OS's rendering of the rows, one each.
    let text = apps::group_text(&notes());
    assert!(
        text.starts_with("## notes\nnotes_add(path: string, text: string)"),
        "{text}"
    );
    assert!(text.contains("\nnotes_count() -> int"), "{text}");
    // No apps, no turn; an app with no rows is left out.
    assert_eq!(apps::turn(&[]).0, "");
    let empty = apps::AppGroup {
        app: "empty".into(),
        rows: std::vec![],
    };
    assert_eq!(
        apps::turn(&[empty]),
        (String::new(), std::vec!["empty".into()])
    );
}

#[test]
fn apps_past_the_budget_are_left_out_and_the_rest_fit_it() {
    let names: std::vec::Vec<&'static str> = (0..40)
        .map(|i| &*std::boxed::Box::leak(std::format!("app{i}_do_something_long").into_boxed_str()))
        .collect();
    let groups: std::vec::Vec<apps::AppGroup> = (0..40)
        .map(|i| apps::AppGroup {
            app: std::format!("app{i}"),
            rows: std::vec![app_row(names[i], "cpus"); 3],
        })
        .collect();
    let (turn, left_out) = apps::turn(&groups);
    assert!(
        estimate_tokens(&turn) <= apps::APPS_BUDGET,
        "{}",
        estimate_tokens(&turn)
    );
    assert!(!left_out.is_empty());
    assert!(turn.contains("\napp0: "));
    assert!(!turn.contains(&std::format!("\n{}: ", left_out[0])));
}

#[test]
fn the_apps_wire_reads_back_whole_entries_only() {
    let answers = [
        ("notes", "version 2\nkai-actions 1\n"),
        ("settings", "version 1\nx\n"),
    ];
    let (text, left_out) = apps::wire(&answers, 1 << 12);
    assert!(left_out.is_empty());
    assert_eq!(apps::entries(&text), Some(answers.to_vec()));
    // An entry that would not fit is left out whole, never cut.
    let (text, left_out) = apps::wire(&answers, 40);
    assert_eq!(left_out, std::vec![String::from("settings")]);
    assert_eq!(apps::entries(&text), Some(std::vec![answers[0]]));
    assert_eq!(apps::entries(""), Some(std::vec![]));
    for bad in [
        "app notes 99\nshort",
        "app notes\nx",
        "app no/tes 1\nx",
        "app  1\nx",
        "apps notes 1\nx",
        "app notes 1",
        "app notes -1\n",
    ] {
        assert_eq!(apps::entries(bad), None, "{bad}");
    }
}
