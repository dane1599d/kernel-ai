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
fn the_flat_apps_turn_shows_every_apps_full_lines() {
    let empty = apps::AppGroup {
        app: "empty".into(),
        rows: std::vec![],
    };
    let (turn, left_out) = apps::flat_turn(&[notes(), empty.clone()]);
    assert_eq!(
        turn,
        std::format!(
            "{}\n{}",
            apps::FLAT_HEADING,
            apps::group_text(&notes()).trim_end()
        )
    );
    assert_eq!(left_out, ["empty"]);
    assert_eq!(apps::flat_turn(&[]), (String::new(), std::vec![]));
    assert_eq!(
        apps::flat_turn(&[empty]),
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

/// Audit (lane M, item 2): row names an app chooses, all digits and lone
/// letters, cost a token a character, three times the bytes estimate. The
/// apps turn counts them so, and leaves such an app out instead of taking
/// about 800 tokens; an app with plain names still fits beside it.
#[test]
fn an_app_whose_row_names_are_digits_is_counted_by_its_digits() {
    let name = "ab_1c2d3e4f5g6h7i8j9k0l1m";
    assert!(apps::estimate(name) >= 20, "{}", apps::estimate(name));
    assert!(
        estimate_tokens(name) < 10,
        "the bytes estimate undercounts it"
    );
    let leaked: &'static str =
        std::boxed::Box::leak(std::string::String::from(name).into_boxed_str());
    let digits = apps::AppGroup {
        app: "ab".into(),
        rows: std::vec![app_row(leaked, "cpus"); 32],
    };
    let (turn, left_out) = apps::turn(&[digits, notes()]);
    assert_eq!(left_out, ["ab"]);
    assert!(turn.contains("\nnotes: notes_add notes_count"), "{turn}");
    assert!(apps::estimate(&turn) <= apps::APPS_BUDGET);
}

/// llm counts with the model's tokenizer: what it says decides, even where an
/// estimate would let the line in.
#[test]
fn the_apps_turn_keeps_to_the_count_it_is_given() {
    let (turn, left_out) = apps::turn(&[notes()]);
    assert!(left_out.is_empty(), "{turn}");
    let strict = |text: &str| 4 * text.len();
    let (turn, left_out) = apps::turn_counted(&[notes()], strict);
    assert_eq!(
        (turn.as_str(), left_out),
        ("", std::vec![String::from("notes")])
    );
}

#[test]
fn a_short_doc_keeps_the_docs_first_words_without_articles() {
    assert_eq!(
        apps::short_doc("The events on a day, like today.", 12),
        "events on day"
    );
    assert_eq!(
        apps::short_doc("Skip to the next song.", 12),
        "skip to next song"
    );
    // A cut leaves no preposition dangling.
    assert_eq!(apps::short_doc("Skip to the next song.", 1), "skip");
    assert_eq!(
        apps::short_doc("Add a contact with a phone number.", 3),
        "add contact"
    );
    assert_eq!(apps::short_doc("", 4), "");
    assert_eq!(apps::short_doc("The.", 4), "");
}

#[test]
fn the_described_turn_cuts_descriptions_to_fit_the_shipped_turns_budget() {
    let doc = |row: &agentabi::Action| apps::short_doc(row.doc, apps::DESCRIBED_MAX_WORDS);
    let group = notes();
    let made = apps::described_turn(core::slice::from_ref(&group), apps::estimate);
    assert_eq!(made.words, apps::DESCRIBED_MAX_WORDS);
    assert_eq!(
        made.text,
        std::format!(
            "{}\nnotes: notes_add ({}) notes_count ({})",
            apps::APPS_HEADING,
            doc(group.rows[0]),
            doc(group.rows[1])
        )
    );
    // A budget that holds the plain lines but not every description cuts
    // every description to fewer words; one that holds no description at
    // all shows the app's plain line, as the shipped turn would.
    let plain = apps::turn(core::slice::from_ref(&group)).0;
    // Ten a word: the plain turn fits; the descriptions are cut to the most
    // words that still fit, and one word more would not.
    let by_words = |text: &str| 10 * text.split_whitespace().count();
    let cut = apps::described_turn(core::slice::from_ref(&group), by_words);
    assert!(cut.plain.is_empty() && cut.left_out.is_empty(), "{cut:?}");
    assert!(by_words(&cut.text) <= apps::APPS_BUDGET, "{cut:?}");
    let more = apps::described_line(&group, cut.words + 1);
    let base = by_words(apps::APPS_HEADING);
    assert!(
        cut.words == apps::DESCRIBED_MAX_WORDS
            || base + by_words(&more) > apps::APPS_BUDGET
            || more == apps::described_line(&group, cut.words),
        "{cut:?}"
    );
    // The app's line costs more than the budget with any description.
    let tight = |text: &str| match text.starts_with("notes:") && text.contains('(') {
        true => apps::APPS_BUDGET + 1,
        false => apps::estimate(text),
    };
    let fallback = apps::described_turn(core::slice::from_ref(&group), tight);
    assert_eq!(fallback.text, plain);
    assert_eq!(fallback.plain, ["notes"]);
    assert_eq!(fallback.words, 1);
    // An app the shipped turn leaves out is left out here too.
    let empty = apps::AppGroup {
        app: "empty".into(),
        rows: std::vec![],
    };
    let made = apps::described_turn(&[empty], apps::estimate);
    assert_eq!(
        (made.text.as_str(), made.left_out),
        ("", std::vec![String::from("empty")])
    );
}
