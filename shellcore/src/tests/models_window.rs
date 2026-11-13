//! The Models window's switches (#556): `use <sha256>` from `desktop`, run as
//! Kai's `model_use` row with the window's click as consent, logged with its
//! undo; anything else from the window, or from another app, refused before
//! `updated` is asked.

use super::*;
use comp_models::hex;

const QWEN: [u8; 32] = [0xaa; 32];
const SMALL: [u8; 32] = [0xbb; 32];
/// The fixture: signed, not stored.
const FIXTURE: [u8; 32] = [0xcc; 32];

fn switch(sha256: &[u8; 32]) -> String {
    format!("use {}", hex(sha256))
}

#[test]
fn a_click_in_the_window_switches_is_logged_as_the_click_and_undo_puts_the_model_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let answer = shell.models_change(crate::MODELS_APP, &switch(&SMALL), true);
    assert!(
        answer
            .as_deref()
            .is_ok_and(|text| text.starts_with("Done: the next restart uses it")),
        "{answer:?}"
    );
    assert_eq!(host.models.borrow().store.next, Some(SMALL));
    let asked = host.model_requests.borrow().clone();
    assert_eq!(asked.last(), Some(&format!("models use {}", hex(&SMALL))));
    let log = shell.eval("log actlog").text;
    assert!(
        log.contains("form=call tier=run consent=user-click:models"),
        "{log}"
    );
    assert!(
        log.contains(&format!(
            "res=model before={} after={}",
            hex(&QWEN),
            hex(&SMALL)
        )),
        "{log}"
    );
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: the model before, from the next restart")
    );
    assert_eq!(host.models.borrow().store.next, None);
}

#[test]
fn a_switch_no_key_or_click_made_is_refused_before_updated_is_asked() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(
        shell.models_change(crate::MODELS_APP, &switch(&SMALL), false),
        Err(String::from("a switch needs a key or a click"))
    );
    assert!(host.model_requests.borrow().is_empty());
    assert_eq!(host.models.borrow().store.next, None);
}

#[test]
fn only_the_models_window_switches() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for app in [crate::SETTINGS_APP, "notes-window", "shell", ""] {
        assert_eq!(
            shell.models_change(app, &switch(&SMALL), true),
            Err(String::from("only the Models window switches the model")),
            "{app}"
        );
    }
    assert!(host.model_requests.borrow().is_empty());
}

#[test]
fn only_a_use_of_a_whole_sha256_is_a_switch() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let small = hex(&SMALL);
    for request in [
        String::from("import"),
        String::from("use qwen3-0.6b"),
        format!("use {}", &small[..12]),
        format!("use {}", small.to_ascii_uppercase()),
        format!("use {small} "),
        format!("use  {small}"),
        format!("model_use {small}"),
        small.clone(),
    ] {
        assert_eq!(
            shell.models_change(crate::MODELS_APP, &request, true),
            Err(String::from("not a model switch")),
            "{request}"
        );
    }
    assert!(host.model_requests.borrow().is_empty());
}

#[test]
fn a_switch_to_a_model_the_role_does_not_take_says_why() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let answer = shell.models_change(crate::MODELS_APP, &switch(&FIXTURE), true);
    assert!(
        answer
            .as_ref()
            .is_err_and(|why| why.contains("not a stored model the signed role takes")),
        "{answer:?}"
    );
    assert_eq!(host.models.borrow().store.next, None);
    // The model running now is not one to switch to.
    let answer = shell.models_change(crate::MODELS_APP, &switch(&QWEN), true);
    assert!(answer.is_err(), "{answer:?}");
}
