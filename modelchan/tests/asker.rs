//! Who may ask `updated` what: the window reads the list, and only that.

use modelchan::asker::{Asker, LIST, SHELL, WINDOW};

const SHA: &str = "73174ba39c9a0000000000000000000000000000000000000000000000000000";

#[test]
fn callers_are_known_by_their_process_names_alone() {
    assert_eq!(Asker::of(SHELL), Asker::Shell);
    assert_eq!(Asker::of(WINDOW), Asker::Window);
    for other in [
        "",
        "Shell",
        "shell ",
        "models",
        "models-window2",
        "monitor-window",
        "settings-window",
        "desktop",
        "comphost",
    ] {
        assert_eq!(Asker::of(other), Asker::Other, "{other:?}");
    }
}

#[test]
fn the_shell_may_send_every_request_updated_answers() {
    for request in [
        LIST,
        "models import",
        "models progress",
        &format!("models use {SHA}"),
    ] {
        assert_eq!(Asker::Shell.may_ask(request), Ok(()), "{request}");
    }
}

#[test]
fn the_window_may_read_the_list() {
    assert_eq!(Asker::Window.may_ask(LIST), Ok(()));
}

#[test]
fn the_window_may_not_use_a_model() {
    let refused = Asker::Window.may_ask(&format!("models use {SHA}"));
    assert_eq!(
        refused,
        Err("the model manager's window may only read the list")
    );
}

#[test]
fn the_window_may_not_import_a_model() {
    assert!(Asker::Window.may_ask("models import").is_err());
}

#[test]
fn the_window_may_not_follow_an_import() {
    assert!(Asker::Window.may_ask("models progress").is_err());
}

#[test]
fn the_window_may_not_send_anything_else() {
    for request in [
        "",
        " models",
        "models ",
        "models\n",
        "MODELS",
        "models models",
        "health",
        "check",
    ] {
        assert!(Asker::Window.may_ask(request).is_err(), "{request:?}");
    }
}

#[test]
fn any_other_caller_may_send_nothing() {
    for request in [LIST, "models import", "models progress", ""] {
        assert_eq!(
            Asker::Other.may_ask(request),
            Err("only the shell and the model manager's window may ask"),
            "{request}"
        );
    }
}
