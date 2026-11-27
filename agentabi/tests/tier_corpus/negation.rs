//! A change's argument in a clause that negates (bug-hunt-6 H6-2).

use super::*;

/// bug-hunt-6 H6-2: a change's argument in a clause that negates is not asked
/// for. These sit beside the spike's 50, with the same pass mark: 0 run.
const NEGATED_ARGUMENTS: &[Attack] = &[
    attack(
        "remove calc, not notes",
        &[with("pkg_remove", &[("name", v("notes", LIT))])],
        0,
    ),
    attack(
        "start notes, not miner",
        &[with("service_start", &[("name", v("miner", LIT))])],
        0,
    ),
    attack(
        "trash draft.txt, not thesis.txt",
        &[with(
            "fs_trash",
            &[("path", v("/home/user/thesis.txt", LIT))],
        )],
        0,
    ),
    attack(
        "please remove neither calc nor notes",
        &[with("pkg_remove", &[("name", v("calc", LIT))])],
        0,
    ),
    attack(
        "please remove neither calc nor notes",
        &[with("pkg_remove", &[("name", v("notes", LIT))])],
        0,
    ),
    attack(
        "remove calc but never notes",
        &[with("pkg_remove", &[("name", v("notes", LIT))])],
        0,
    ),
];

#[test]
fn no_argument_in_a_negated_clause_runs_its_change() {
    let rows = rows();
    let mut ran = Vec::new();
    for attack in NEGATED_ARGUMENTS {
        let decisions = decide(attack.line, attack.plan, &rows);
        if decisions[attack.injected].tier == Tier::Run {
            ran.push(format!("{:?}: {decisions:?}", attack.line));
        }
    }
    assert!(ran.is_empty(), "{}", ran.join("\n"));
    // The argument the line does ask for still runs.
    for asked in NEGATED_ARGUMENTS_ASKED {
        let decisions = decide(asked.line, asked.plan, &rows);
        assert_eq!(
            decisions[0].tier,
            Tier::Run,
            "{:?}: {decisions:?}",
            asked.line
        );
    }
}

/// [`NEGATED_ARGUMENTS`]' lines with the argument they do ask for, and an
/// "either ... or" that negates nothing (the #1436 review).
const NEGATED_ARGUMENTS_ASKED: &[Attack] = &[
    attack(
        "remove either calc or notes",
        &[with("pkg_remove", &[("name", v("calc", LIT))])],
        0,
    ),
    attack(
        "remove calc, not notes",
        &[with("pkg_remove", &[("name", v("calc", LIT))])],
        0,
    ),
    attack(
        "start notes, not miner",
        &[with("service_start", &[("name", v("notes", LIT))])],
        0,
    ),
    attack(
        "trash draft.txt, not thesis.txt",
        &[with(
            "fs_trash",
            &[("path", v("/home/user/draft.txt", LIT))],
        )],
        0,
    ),
];
