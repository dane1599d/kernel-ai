//! Everyday words for a file or the theme (founder test 2).

use super::*;

const IDEAS: Value<'static> = v("/home/user/ideas", LIT);
const NO_TEXT: Value<'static> = v("", LIT);

/// Everyday words for a file or the theme (founder test 2, item 4): each
/// names the change it asks for, so it runs, with no card.
const EVERYDAY_LINES: &[Case] = &[
    case(
        "make a note called ideas",
        &[with("fs_create", &[("path", IDEAS), ("text", NO_TEXT)])],
        Runs,
    ),
    case(
        "new file called ideas",
        &[with("fs_create", &[("path", IDEAS), ("text", NO_TEXT)])],
        Runs,
    ),
    case(
        "save hello in a file called ideas",
        &[with(
            "fs_create",
            &[("path", IDEAS), ("text", v("hello", LIT))],
        )],
        Runs,
    ),
    case(
        "remove the file ideas",
        &[with("fs_trash", &[("path", IDEAS)])],
        Runs,
    ),
    case("erase ideas", &[with("fs_trash", &[("path", IDEAS)])], Runs),
    case(
        "open the file manager",
        &[with("ui_open", &[("app", v("files", LIT))])],
        Runs,
    ),
    case(
        "dark mode please",
        &[with(
            "appearance_set",
            &[("setting", v("theme", LIT)), ("value", v("dark", LIT))],
        )],
        Runs,
    ),
    case(
        "switch to light mode",
        &[with(
            "appearance_set",
            &[("setting", v("theme", LIT)), ("value", v("light", LIT))],
        )],
        Runs,
    ),
];

/// The same words in a line that asks to be told: no change is asked for
/// (#721), whatever the cues.
const EVERYDAY_TELLING: &[Case] = &[
    case(
        "tell me how to remove ideas",
        &[with("fs_trash", &[("path", IDEAS)])],
        Asks,
    ),
    case(
        "explain how to make a new note called ideas",
        &[with("fs_create", &[("path", IDEAS), ("text", NO_TEXT)])],
        Asks,
    ),
    case(
        "show me how to erase ideas",
        &[with("fs_trash", &[("path", IDEAS)])],
        Asks,
    ),
    case(
        "what would dark mode look like?",
        &[with(
            "appearance_set",
            &[("setting", v("theme", LIT)), ("value", v("dark", LIT))],
        )],
        Asks,
    ),
    // A complaint beside a telling line takes no vague reading (#883 review):
    // the line names an action, so the change it did not ask for asks.
    case(
        "tell me how to remove ideas, it's so slow",
        &[with("fs_trash", &[("path", IDEAS)])],
        Asks,
    ),
    case(
        "explain how to switch to round robin, it's laggy",
        &[RR],
        Asks,
    ),
    // So does a complaint beside a question (a review of #886): the question's
    // cue names the action though it asks for none.
    case("it's so slow, how do I switch to round robin", &[RR], Asks),
    case(
        "tell me, how do I switch to round robin, it's laggy",
        &[RR],
        Asks,
    ),
    case(
        "explain: how do I switch to round robin? it's slow",
        &[RR],
        Asks,
    ),
    case(
        "it's so slow, how do I remove ideas",
        &[with("fs_trash", &[("path", IDEAS)])],
        Asks,
    ),
    // "model" is not the theme's "mode".
    case(
        "switch the model",
        &[with(
            "appearance_set",
            &[("setting", v("theme", LIT)), ("value", v("dark", LIT))],
        )],
        Asks,
    ),
];

#[test]
fn everyday_words_in_a_telling_line_ask_for_nothing() {
    let rows = rows();
    for case in EVERYDAY_TELLING {
        let decisions = decide(case.line, case.plan, &rows);
        assert_eq!(
            outcome(&decisions),
            case.expect,
            "{:?}: {decisions:?}",
            case.line
        );
        assert_eq!(decisions[0].why, Why::Added, "{:?}", case.line);
    }
}

#[test]
fn everyday_words_for_a_change_run_it() {
    let rows = rows();
    for case in EVERYDAY_LINES {
        let decisions = decide(case.line, case.plan, &rows);
        assert_eq!(
            outcome(&decisions),
            case.expect,
            "{:?}: {decisions:?}",
            case.line
        );
    }
}
