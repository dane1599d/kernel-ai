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
    for row in agentabi::SCRIPT_API.iter().chain(&agentabi::COMMANDS) {
        assert!(
            pack.contains(&std::format!("\n{}(", row.name)),
            "{}",
            row.name
        );
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
