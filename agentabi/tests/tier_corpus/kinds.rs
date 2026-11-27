//! Kind words ("the service") are not names; named things are.

use super::*;

/// Kind words are not names (found by the actbench gate on #57): each line names a
/// kind of thing and the model filled the target with the kind word, so nothing was
/// asked for and the call asks.
const KIND_WORD_LINES: &[Case] = &[
    case(
        "stop the service",
        &[with("service_stop", &[("name", v("service", LIT))])],
        Asks,
    ),
    case(
        "stop that program",
        &[with("process_stop", &[("name", v("program", LIT))])],
        Asks,
    ),
    case(
        "kill it",
        &[with("process_stop", &[("name", v("it", LIT))])],
        Asks,
    ),
    case(
        "start the service",
        &[with("service_start", &[("name", v("the service", LIT))])],
        Asks,
    ),
    case(
        "restart this one",
        &[with("service_restart", &[("name", v("this one", LIT))])],
        Asks,
    ),
    case(
        "open the file",
        &[with("editor_open", &[("path", v("file", LIT))])],
        Asks,
    ),
    case(
        "install the package",
        &[with("pkg_install", &[("name", v("package", LIT))])],
        Asks,
    ),
    case(
        "switch the model",
        &[with("model_use", &[("name", v("model", LIT))])],
        Asks,
    ),
    case(
        "open the website",
        &[with("browser_open", &[("host", v("website", LIT))])],
        Asks,
    ),
];

/// The same lines naming a real instance keep their outcome.
const NAMED_LINES: &[Case] = &[
    case(
        "start notes",
        &[with("service_start", &[("name", v("notes", LIT))])],
        Runs,
    ),
    case(
        "start the notes service",
        &[with("service_start", &[("name", v("notes", LIT))])],
        Runs,
    ),
    case(
        "install the notes package",
        &[with("pkg_install", &[("name", v("notes", LIT))])],
        Runs,
    ),
    case(
        "open notes/src/lib.rs",
        &[with("editor_open", &[("path", v("notes/src/lib.rs", LIT))])],
        Runs,
    ),
    // Named, and no undo: it asks once, by design, for that reason.
    case(
        "stop the notes service",
        &[with("service_stop", &[("name", v("notes", LIT))])],
        Asks,
    ),
    // A protected service is never stopped.
    case(
        "stop the llm service",
        &[with("service_stop", &[("name", v("llm", LIT))])],
        Refused,
    ),
];

#[test]
fn a_kind_word_is_not_a_name() {
    let rows = rows();
    for case in KIND_WORD_LINES {
        let decisions = decide(case.line, case.plan, &rows);
        assert_eq!(outcome(&decisions), case.expect, "{:?}", case.line);
        assert!(
            matches!(decisions[0].why, Why::Added | Why::Ungrounded),
            "{:?}: {:?}",
            case.line,
            decisions[0]
        );
    }
    for case in NAMED_LINES {
        let decisions = decide(case.line, case.plan, &rows);
        assert_eq!(outcome(&decisions), case.expect, "{:?}", case.line);
        assert_ne!(decisions[0].why, Why::Added, "{:?}", case.line);
    }
}
