//! The `model_*` rows (P4 task 10): the list, `model_use` with its undo, and
//! `model_fetch`, which asks, carried out by `updated` through `Host::models`.

use super::*;
use alloc::vec;
use comp_models::{Models, Signed, Store, Stored, hex};

const QWEN: [u8; 32] = [0xaa; 32];
const SMALL: [u8; 32] = [0xbb; 32];
const FIXTURE: [u8; 32] = [0xcc; 32];
const UNSIGNED: [u8; 32] = [0xdd; 32];

fn signed(id: &str, name: &str, sha256: [u8; 32], length: u64) -> Signed {
    Signed {
        id: id.into(),
        name: name.into(),
        length,
        sha256,
        licence: "Apache-2.0".into(),
        source: String::new(),
    }
}

/// What the fake `updated` starts with: Qwen3-4B runs; the small model and a
/// blob the role does not name are stored; the fixture is signed, not stored.
pub(super) fn store() -> Models {
    Models {
        signed: vec![
            signed("qwen3-4b-2507", "Qwen3-4B Q4_K_M", QWEN, 2_497_281_120),
            signed("qwen3-0.6b", "Qwen3-0.6B Q8_0", SMALL, 639_446_688),
            signed("fixture", "Fixture model", FIXTURE, 4096),
        ],
        store: Store {
            blobs: vec![
                Stored {
                    sha256: QWEN,
                    length: 2_497_281_120,
                },
                Stored {
                    sha256: SMALL,
                    length: 639_446_688,
                },
                Stored {
                    sha256: UNSIGNED,
                    length: 77,
                },
            ],
            running: Some(QWEN),
            ..Store::default()
        },
        ..Models::default()
    }
}

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

#[test]
fn the_list_names_each_model_with_its_licence_and_state() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"model_list","arguments":{}}"#));
    let reply = shell.eval("which AI models do I have");
    assert!(reply.ok, "{reply:?}");
    for line in [
        "Qwen3-4B Q4_K_M (qwen3-4b-2507): in use, 2.3 GiB, Apache-2.0",
        "Qwen3-0.6B Q8_0 (qwen3-0.6b): stored, 609.8 MiB, Apache-2.0",
        "Fixture model (fixture): signed, not stored, 4.0 KiB, Apache-2.0",
        "not in the signed role",
    ] {
        assert!(reply.text.contains(line), "{line} in {}", reply.text);
    }
    assert_eq!(*host.model_requests.borrow(), ["models"]);
}

#[test]
fn with_no_model_store_the_list_says_why() {
    let host = FakeHost::new();
    *host.model_answer.borrow_mut() = Some("err there is no model store: no data disk".into());
    let mut shell = shell(&host);
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"model_list","arguments":{}}"#));
    let reply = shell.eval("which AI models do I have");
    assert!(
        !reply.ok && reply.text.contains("there is no model store: no data disk"),
        "{reply:?}"
    );
}

#[test]
fn a_named_switch_runs_and_undo_keeps_the_model_before() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"model_use","arguments":{"name":"qwen3-0.6b"}}"#,
    ));
    let reply = shell.eval("please switch the model to qwen3-0.6b");
    assert!(
        reply.ok && reply.text.starts_with("Done: the next restart uses it"),
        "{reply:?}"
    );
    assert_eq!(host.models.borrow().store.next, Some(SMALL));
    let asked = host.model_requests.borrow().clone();
    assert_eq!(asked.last(), Some(&format!("models use {}", hex(&SMALL))));
    assert!(shell.eval("log actlog").text.contains(&format!(
        "res=model before={} after={}",
        hex(&QWEN),
        hex(&SMALL)
    )));
    let undone = shell.eval("undo");
    assert_eq!(
        undone,
        ok("Undone: the model before, from the next restart")
    );
    assert_eq!(
        host.models.borrow().store.next,
        None,
        "the trial is cancelled"
    );
    let asked = host.model_requests.borrow().clone();
    assert_eq!(asked.last(), Some(&format!("models use {}", hex(&QWEN))));
}

#[test]
fn undo_after_another_switch_asks_before_putting_the_model_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"model_use","arguments":{"name":"qwen3-0.6b"}}"#,
    ));
    shell.eval("please switch the model to qwen3-0.6b");
    // Another model for the next restart since, not this shell's switch.
    host.models.borrow_mut().store.next = Some(UNSIGNED);
    let now = &hex(&UNSIGNED)[..12];
    let asked = shell.eval("undo");
    assert!(
        asked.text.contains(&format!("changed since (now {now})")) && asked.text.ends_with("y/n"),
        "{asked:?}"
    );
    let sent = host.model_requests.borrow().len();
    assert_eq!(shell.eval("n"), ok("Not undone; `undo` asks again"));
    assert_eq!(
        host.model_requests.borrow().len(),
        sent,
        "nothing was asked"
    );
    shell.eval("undo");
    assert_eq!(
        shell.eval("Yes"),
        ok("Undone: the model before, from the next restart")
    );
    let asked = host.model_requests.borrow().clone();
    assert_eq!(asked.last(), Some(&format!("models use {}", hex(&QWEN))));
    assert_eq!(
        host.models.borrow().store.next,
        None,
        "the running model again"
    );
}

#[test]
fn only_a_stored_signed_model_is_used() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let unsigned = &hex(&UNSIGNED)[..12];
    // The running model by its hash: "qwen3-4b-2507" in a line reads as code
    // to the shell's line classifier (A-core's to look at).
    let running = &hex(&QWEN)[..12];
    for (name, why) in [
        ("fixture", "not a stored model the signed role takes"),
        (unsigned, "not a stored model the signed role takes"),
        (running, "the next restart uses it already"),
        ("nothing", "no model nothing"),
    ] {
        let json = format!(r#"{{"name":"model_use","arguments":{{"name":"{name}"}}}}"#);
        host.calls.borrow_mut().push_back(call(&json));
        let reply = shell.eval(&format!("please switch the model to {name}"));
        assert!(!reply.ok && reply.text.contains(why), "{name}: {reply:?}");
    }
    assert!(
        host.model_requests
            .borrow()
            .iter()
            .all(|r| !r.starts_with("models use")),
        "nothing was switched"
    );
}

#[test]
fn a_model_the_ai_picked_asks_once() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"model_use","arguments":{"name":"qwen3-0.6b"}}"#,
    ));
    let reply = shell.eval("please switch to a smaller model");
    assert!(reply.text.contains("model_use"), "{reply:?}");
    assert_eq!(host.models.borrow().store.next, None, "nothing yet");
    let once = host.mint(1, "model_use(\"qwen3-0.6b\")");
    let answered = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(
        answered.text.starts_with("Done: the next restart uses it"),
        "{answered:?}"
    );
    assert_eq!(host.models.borrow().store.next, Some(SMALL));
}

#[test]
fn an_import_asks_and_runs_only_on_an_allow() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let fetch = r#"{"name":"model_fetch","arguments":{"source":"disk"}}"#;
    host.calls.borrow_mut().push_back(call(fetch));
    let reply = shell.eval("please import the model from the attached disk");
    assert!(reply.text.contains("model_fetch"), "{reply:?}");
    assert!(
        !host
            .model_requests
            .borrow()
            .contains(&"models import".into())
    );
    shell.eval("n");
    assert!(
        !host
            .model_requests
            .borrow()
            .contains(&"models import".into())
    );
    host.calls.borrow_mut().push_back(call(fetch));
    shell.eval("please import the model from the attached disk");
    let once = host.mint(2, "model_fetch(\"disk\")");
    let answered = shell.eval(&format!("answer 2 allow once={once}"));
    assert_eq!(
        answered,
        ok(
            "Importing fixture (4.0 KiB) from the attached disk, checked against the signed role; model_list shows when it is stored"
        )
    );
    assert!(
        host.model_requests
            .borrow()
            .contains(&"models import".into())
    );
}

#[test]
fn a_model_inverse_reads_back_from_its_word() {
    use crate::undo::{Inverse, inverse_word, parse_inverse};
    for inverse in [
        Inverse::Model {
            back: Some(QWEN),
            from: SMALL,
        },
        Inverse::Model {
            back: None,
            from: SMALL,
        },
    ] {
        let word = inverse_word(Some(inverse));
        assert_eq!(parse_inverse(&word), Some(Some(inverse)), "{word}");
    }
}

// A person's typed `models` lines (P4 task 19): the same rows, with no model.

#[test]
fn a_typed_models_lists_without_asking_the_model() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let reply = shell.eval("models");
    assert!(
        reply.ok && reply.text.contains("Qwen3-0.6B Q8_0 (qwen3-0.6b): stored"),
        "{reply:?}"
    );
    // The list, then what may be downloaded (P5 task 9).
    assert_eq!(*host.model_requests.borrow(), ["models", "models offers"]);
    assert!(host.calls.borrow().is_empty(), "no model was asked");
}

#[test]
fn a_typed_switch_runs_is_logged_as_typed_and_undo_puts_the_model_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let reply = shell.eval("models use qwen3-0.6b");
    assert!(
        reply.ok && reply.text.starts_with("Done: the next restart uses it"),
        "{reply:?}"
    );
    assert_eq!(host.models.borrow().store.next, Some(SMALL));
    let log = shell.eval("log actlog").text;
    assert!(log.contains("form=call tier=run consent=typed"), "{log}");
    assert!(
        log.contains(&format!("res=model before={}", hex(&QWEN))),
        "{log}"
    );
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: the model before, from the next restart")
    );
    assert_eq!(host.models.borrow().store.next, None);
    // A model the role does not take is refused, as for the AI.
    let reply = shell.eval("models use fixture");
    assert!(
        !reply.ok
            && reply
                .text
                .contains("not a stored model the signed role takes"),
        "{reply:?}"
    );
}

#[test]
fn a_typed_import_asks_once_and_runs_only_on_an_allow() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let reply = shell.eval("models import");
    assert!(reply.text.contains("model_fetch"), "{reply:?}");
    let imported = |host: &FakeHost| {
        host.model_requests
            .borrow()
            .contains(&"models import".into())
    };
    assert!(!imported(&host), "nothing before the answer");
    shell.eval("n");
    assert!(!imported(&host), "a Deny imports nothing");
    shell.eval("models import");
    let once = host.mint(2, "model_fetch(\"disk\")");
    let answered = shell.eval(&format!("answer 2 allow once={once}"));
    assert!(
        answered.text.starts_with("Importing fixture"),
        "{answered:?}"
    );
    assert!(imported(&host));
    assert!(host.calls.borrow().is_empty(), "no model was asked");
}

#[test]
fn the_typed_list_offers_the_signed_models_this_machine_may_download() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let reply = shell.eval("models");
    assert!(reply.ok, "{reply:?}");
    for line in [
        "Fixture model (fixture): signed, not stored",
        "To download one: models download <id>",
        "  fixture-dl: fits, recommended",
        "  qwen3-8b: it needs 6208 MiB of memory and this machine has 4096 MiB",
    ] {
        assert!(reply.text.contains(line), "{line} in {}", reply.text);
    }
    // A stored target is not offered again.
    assert!(!reply.text.contains("  qwen3-4b-2507:"), "{}", reply.text);
    assert_eq!(*host.model_requests.borrow(), ["models", "models offers"]);
    // An `updated` that cannot say does not hide the list, and says why.
    *host.model_answer.borrow_mut() = Some(String::from("err no data disk"));
    let reply = shell.eval("models");
    assert!(
        reply.text.contains("Downloads: none listed (no data disk)") || !reply.ok,
        "{reply:?}"
    );
}

#[test]
fn a_typed_download_asks_once_with_its_size_and_host_and_runs_only_on_an_allow() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let downloaded = |host: &FakeHost| {
        host.model_requests
            .borrow()
            .contains(&"models download fixture-dl".into())
    };
    let reply = shell.eval("models download fixture-dl");
    assert_eq!(reply.confirm, Some(1), "{reply:?}");
    // Question 6: the size and the host, before the call that names the model.
    assert!(
        reply.text.contains(
            "Download 12.0 MiB from models.kai.test with model_fetch(\"net\", \"fixture-dl\")? It has no undo."
        ),
        "{reply:?}"
    );
    assert!(!downloaded(&host), "nothing before the answer");
    shell.eval("n");
    assert!(!downloaded(&host), "a Deny downloads nothing");
    shell.eval("models download fixture-dl");
    let once = host.mint(2, "model_fetch(\"net\", \"fixture-dl\")");
    let answered = shell.eval(&format!("answer 2 allow once={once}"));
    assert!(
        answered.ok
            && answered.text.starts_with(
                "Downloading fixture-dl, 12.0 MiB from models.kai.test; each 2 MiB part is checked"
            ),
        "{answered:?}"
    );
    assert!(downloaded(&host));
    let log = shell.eval("log actlog").text;
    assert!(
        log.contains(
            "model_fetch owner=updated res=model-store before=- after=fixture-dl inv=none"
        ),
        "{log}"
    );
    assert!(host.calls.borrow().is_empty(), "no model was asked");
}

#[test]
fn a_download_updated_refuses_is_said_and_never_asked() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let reply = shell.eval("models download qwen3-99b");
    assert!(
        !reply.ok
            && reply.text
                == "qwen3-99b is not downloaded: the signed models role names no qwen3-99b",
        "{reply:?}"
    );
    assert_eq!(reply.confirm, None);
    assert!(
        !host
            .model_requests
            .borrow()
            .iter()
            .any(|r| r.starts_with("models download")),
        "nothing started"
    );
}

#[test]
fn a_models_line_that_is_not_right_says_what_is_and_a_variable_stays_rhai() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for text in [
        "models delete qwen3-0.6b",
        "models use",
        "models import now",
        "models download",
        "models download a b",
    ] {
        let reply = shell.eval(text);
        assert!(
            !reply.ok
                && reply
                    .text
                    .starts_with("models, models import, models download <id> or models use <id>"),
            "{text}: {reply:?}"
        );
    }
    assert!(host.model_requests.borrow().is_empty());
    shell.eval("let models = 3");
    assert_eq!(shell.eval("models"), ok("3"));
    assert!(host.model_requests.borrow().is_empty());
}
