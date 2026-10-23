extern crate alloc;
extern crate std;

use super::*;
use alloc::format;
use alloc::string::ToString;

#[test]
fn every_row_passes_the_check_one_by_one() {
    for row in &ROWS {
        assert_eq!(check_row(row), Ok(()), "{}", row.name);
    }
    assert_eq!(check(&ROWS), Ok(()));
}

#[test]
fn the_lists_cover_every_row_once() {
    let listed = SCRIPT_API.iter().chain(&HELPERS).chain(&COMMANDS);
    assert_eq!(listed.clone().count(), ROWS.len());
    for row in listed {
        assert!(ROWS.iter().any(|r| r.name == row.name), "{}", row.name);
    }
}

/// #15's table (`shellcore/src/tiers.rs` on main at 57d7120): the generated one must
/// give the same tiers.
#[test]
fn derived_tiers_match_the_seamless_actions_table() {
    let before = [
        ("cpus", Tier::Run),
        ("memory", Tier::Run),
        ("uptime_ms", Tier::Run),
        ("processes", Tier::Run),
        ("threads", Tier::Run),
        ("get_policy", Tier::Run),
        ("set_policy", Tier::Run),
        ("fixed", Tier::Run),
        ("floor", Tier::Run),
    ];
    let rows = SCRIPT_API.iter().chain(&HELPERS);
    let now: alloc::vec::Vec<_> = rows.map(|row| (row.name, derive(row))).collect();
    assert_eq!(now, before);
    assert_eq!(derive(&TUNE), Tier::Ask);
    assert_eq!(derive(&TUNE_UNDO), Tier::Run);
    assert_eq!(derive(&COMPONENT_REVERT), Tier::Run);
}

#[test]
fn signatures_are_the_ones_the_prompt_shows() {
    let shown = |row: &Action| row.signature().to_string();
    assert_eq!(shown(&CPUS), "cpus() -> int");
    assert_eq!(
        shown(&MEMORY),
        "memory() -> #{free_mib: int, total_mib: int, heap_kib: int}"
    );
    assert_eq!(
        shown(&PROCESSES),
        "processes() -> [#{pid: int, name: string}]"
    );
    assert_eq!(shown(&UPTIME_MS), "uptime_ms() -> int");
    assert_eq!(
        shown(&THREADS),
        "threads() -> #{switches: int, ejections: int, migrations: int}"
    );
    assert_eq!(shown(&GET_POLICY), "get_policy() -> string");
    assert_eq!(shown(&SET_POLICY), "set_policy(name: string)");
    assert_eq!(shown(&FIXED), "fixed(n: int, places: int) -> string");
    assert_eq!(
        shown(&COMPONENT_REVERT),
        "component_revert(component: string, back: int?)"
    );
}

fn refused(row: Action) -> &'static str {
    check_row(&row).expect_err(row.name)
}

#[test]
fn inconsistent_rows_are_refused() {
    let read_with_undo = Action {
        inverse: Inverse::Restore,
        ..CPUS
    };
    assert!(refused(read_with_undo).contains("inverse Read"));
    let change_as_read = Action {
        inverse: Inverse::Read,
        ..SET_POLICY
    };
    assert!(refused(change_as_read).contains("undone"));
    let gentle_no_undo = Action {
        inverse: Inverse::None("it is gone"),
        ..SET_POLICY
    };
    assert!(refused(gentle_no_undo).contains("destructive"));
    let closed_host = Action {
        args: &[Arg {
            name: "site",
            ty: ArgType::Host,
            role: Role::Target,
            from_request: true,
            never: &[],
            aliases: &[],
        }],
        ..SET_POLICY
    };
    assert!(refused(closed_host).contains("Host"));
    let never_alias = Action {
        args: &[Arg {
            name: "name",
            ty: ArgType::Enum(POLICIES),
            role: Role::Target,
            from_request: true,
            never: &["rr"],
            aliases: &[("rr", "round-robin")],
        }],
        ..SET_POLICY
    };
    assert!(refused(never_alias).contains("alias"));
    let content_as_text = Action {
        returns: Returns::Content("note"),
        ..CPUS
    };
    assert!(refused(content_as_text).contains("Data"));
    for name in [
        "",
        "Cpus",
        "cpus_",
        "cpu__count",
        "1cpus",
        "a_very_long_action_name_x",
    ] {
        assert!(
            refused(Action { name, ..CPUS }).contains("snake_case"),
            "{name:?}"
        );
    }
    let long_doc = format!("{:1$}", "x", MAX_DOC + 1);
    let long_doc: &'static str = alloc::boxed::Box::leak(long_doc.into_boxed_str());
    assert!(
        refused(Action {
            doc: long_doc,
            ..CPUS
        })
        .contains("MAX_DOC bytes")
    );
    assert_eq!(check(&[CPUS, CPUS]), Err("two rows have one name"));
}

#[test]
fn tiers_are_derived_and_min_tier_only_raises() {
    let no_undo = Action {
        facts: Facts::PESSIMISTIC,
        inverse: Inverse::None("sent"),
        ..SET_POLICY
    };
    assert_eq!(derive(&no_undo), Tier::Ask);
    let fetch = Action {
        facts: Facts {
            open_world: true,
            ..Facts::LOCAL_READ
        },
        ..CPUS
    };
    assert_eq!(derive(&fetch), Tier::Ask);
    let held = Action {
        inverse: Inverse::Hold { ms: 10_000 },
        ..SET_POLICY
    };
    assert_eq!(derive(&held), Tier::Ask);
    let lowered = Action {
        min_tier: Tier::Run,
        ..no_undo
    };
    assert_eq!(derive(&lowered), Tier::Ask);
    let never = Action {
        min_tier: Tier::Never,
        ..CPUS
    };
    assert_eq!(derive(&never), Tier::Never);
}

#[test]
fn rows_are_found_by_name_or_old_name() {
    let renamed = [Action {
        name: "sched_set_policy",
        old_name: Some("set_policy"),
        ..SET_POLICY
    }];
    assert!(find(&renamed, "set_policy").is_some());
    assert!(find(&renamed, "sched_set_policy").is_some());
    assert!(find(&ROWS, "nothing").is_none());
}
