//! A site the person allowed for an app grounds Kai's fetch from that one
//! host (ADR 0002 D3; founder decision 2026-11-16): only while the app is
//! installed and the grant is on file, only for a fetch the line alone
//! decided, and never as a destination for data.

use super::act::with_calls;
use super::*;
use crate::ui::UiCall;
use alloc::boxed::Box;
use alloc::vec;
use kaigrants::{From, Grant, Grants, Kind, Life, Time};

const WEATHER: &str = "api.weather.example";
const LINE: &str = "what's the weather in Leeds?";

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn fetch(host: &str, path: Option<&str>) -> Result<Call, ActError> {
    let call = serde_json::json!({
        "name": "net_fetch",
        "arguments": {"host": host, "path": path},
    });
    Ok(llmproto::act::parse(&call.to_string()).unwrap())
}

/// The grants file holding `grants`, each (app, host, life).
fn file(grants: &[(&str, &str, Life)]) -> String {
    let grants = grants
        .iter()
        .enumerate()
        .map(|(at, &(app, object, life))| Grant {
            id: at as u32 + 1,
            app: app.into(),
            kind: Kind::Host,
            object: object.into(),
            rights: vec!["resolve".into(), "connect".into()],
            from: From::Request(7),
            life,
            signer: None,
            made: Time::from_ns(0),
        })
        .collect();
    Grants {
        grants,
        rules: Vec::new(),
    }
    .text()
    .unwrap()
}

/// A shell whose model calls `calls`, with the weather app installed and
/// `grants` on file.
fn granted(
    calls: Vec<Result<Call, ActError>>,
    grants: &[(&str, &str, Life)],
) -> (Rc<FakeHost>, Shell) {
    let (host, shell) = with_calls(calls);
    host.installed
        .borrow_mut()
        .push(("weather".into(), "kai-actions 1".into()));
    *host.net_disk.borrow_mut() = Some(file(grants));
    (host, shell)
}

fn always(app: &'static str) -> [(&'static str, &'static str, Life); 1] {
    [(app, WEATHER, Life::Persistent)]
}

/// The line ran with no ask, and fetched from the weather host.
fn ran(host: &FakeHost, reply: &Reply) {
    assert!(reply.ok && reply.confirm.is_none(), "{reply:?}");
    assert_eq!(*host.net_seen.borrow(), [format!("net_fetch {WEATHER}")]);
}

/// The line asked, and nothing was fetched.
fn asked(host: &FakeHost, reply: &Reply) {
    assert!(reply.confirm.is_some(), "{reply:?}");
    assert!(host.net_seen.borrow().is_empty(), "{reply:?}");
}

#[test]
fn an_apps_grant_grounds_a_plain_fetch_from_its_host() {
    let (host, mut shell) = granted(vec![fetch(WEATHER, None)], &always("weather"));
    ran(&host, &shell.eval(LINE));
    // With no grant on file the same line asks, as before.
    let (bare, mut shell) = with_calls(vec![fetch(WEATHER, None)]);
    asked(&bare, &shell.eval(LINE));
}

#[test]
fn a_windows_grant_counts_for_its_package() {
    // Desktop records the process, `weather-window`; pkg knows `weather`.
    let (host, mut shell) = granted(vec![fetch(WEATHER, None)], &always("weather-window"));
    ran(&host, &shell.eval(LINE));
    assert!(host.installed_asked.borrow().contains(&"weather".into()));
}

#[test]
fn a_this_session_grant_grounds_until_the_next_boot() {
    let session = [("weather", WEATHER, Life::Session)];
    let (host, mut shell) = granted(vec![fetch(WEATHER, None), fetch(WEATHER, None)], &session);
    ran(&host, &shell.eval(LINE));
    crate::netask::at_boot(host.as_ref());
    host.net_seen.borrow_mut().clear();
    asked(&host, &shell.eval(LINE));
}

#[test]
fn a_revoked_grant_grounds_nothing_from_the_next_line() {
    let (host, mut shell) = granted(
        vec![fetch(WEATHER, None), fetch(WEATHER, None)],
        &always("weather"),
    );
    ran(&host, &shell.eval(LINE));
    // Settings' revoke writes the file without it.
    *host.net_disk.borrow_mut() = Some(file(&[]));
    host.net_seen.borrow_mut().clear();
    asked(&host, &shell.eval(LINE));
}

#[test]
fn an_uninstalled_apps_grant_grounds_nothing() {
    let (host, mut shell) = granted(vec![fetch(WEATHER, None)], &always("radar"));
    asked(&host, &shell.eval(LINE));
}

#[test]
fn a_pkg_that_cannot_say_counts_the_grant_for_nothing() {
    let (host, mut shell) = granted(vec![fetch(WEATHER, None)], &always("weather"));
    *host.installed_down.borrow_mut() = Some("pkg did not answer: timed out".into());
    asked(&host, &shell.eval(LINE));
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|a| a.contains("weather's grants count for nothing")),
        "{:?}",
        host.audit.borrow()
    );
}

#[test]
fn a_bad_grants_file_grounds_nothing_and_is_audited_once() {
    let (host, mut shell) = granted(vec![fetch(WEATHER, None), fetch(WEATHER, None)], &[]);
    *host.net_disk.borrow_mut() = Some(String::from("kai-grants 1\nnot a line\n"));
    asked(&host, &shell.eval(LINE));
    assert!(shell.eval("n").ok);
    asked(&host, &shell.eval(LINE));
    let said = host
        .audit
        .borrow()
        .iter()
        .filter(|a| a.starts_with("agent grounding: the network grants on disk are out of shape"))
        .count();
    assert_eq!(said, 1, "{:?}", host.audit.borrow());
}

#[test]
fn a_grant_that_is_no_plain_host_grounds_nothing() {
    use nethosts::Granted;
    let host = FakeHost::new();
    host.installed
        .borrow_mut()
        .push(("weather".into(), "kai-actions 1".into()));
    let said = core::cell::RefCell::new(None);
    for object in ["any", "none", "localhost"] {
        *host.net_disk.borrow_mut() = Some(file(&[
            ("weather", object, Life::Persistent),
            ("weather", WEATHER, Life::Persistent),
        ]));
        let grants = crate::appgrants::AppGrants::new(host.as_ref(), &said);
        if let Ok(parsed) = nethosts::Host::parse(object) {
            assert!(!grants.granted(&parsed), "{object}");
        }
        // The file was read, and its plain host still counts.
        assert!(
            grants.granted(&nethosts::Host::parse(WEATHER).unwrap()),
            "{object}"
        );
    }
}

#[test]
fn a_typed_host_never_reads_the_grants() {
    let (host, mut shell) = granted(vec![fetch("example.com", None)], &always("weather"));
    *host.net_disk.borrow_mut() = Some(String::from("kai-grants 1\nnot a line\n"));
    let reply = shell.eval("fetch example.com");
    assert!(reply.ok && reply.confirm.is_none(), "{reply:?}");
    assert!(host.installed_asked.borrow().is_empty());
    assert!(
        host.audit
            .borrow()
            .iter()
            .all(|a| !a.starts_with("agent grounding"))
    );
}

#[test]
fn a_line_that_says_to_stay_offline_bars_the_grant() {
    let (host, mut shell) = granted(vec![fetch(WEATHER, None)], &always("weather"));
    asked(
        &host,
        &shell.eval("don't go online, just guess the weather in Leeds"),
    );
}

#[test]
fn a_path_the_line_does_not_hold_asks_and_says_why() {
    // The weather app's window title in the State line could read "kai: check
    // api.weather.example/?s=…": a path the line does not hold asks.
    let (host, mut shell) = granted(
        vec![fetch(WEATHER, Some("/?s=meeting at 3"))],
        &always("weather"),
    );
    let reply = shell.eval(LINE);
    asked(&host, &reply);
    assert!(reply.text.contains("or what to read from it"), "{reply:?}");
    assert!(!reply.text.contains("send"), "nothing is sent: {reply:?}");
    // A path whose words are the line's runs.
    let (host, mut shell) = granted(vec![fetch(WEATHER, Some("/leeds"))], &always("weather"));
    let reply = shell.eval(LINE);
    assert!(reply.ok && reply.confirm.is_none(), "{reply:?}");
    assert_eq!(
        *host.net_seen.borrow(),
        [format!("net_fetch {WEATHER}/leeds")]
    );
}

#[test]
fn a_fetch_a_view_decided_asks_and_offers_no_always() {
    let (host, mut shell) = granted(
        vec![
            call(r#"{"name":"ui_view","arguments":{"window":"notes"}}"#),
            fetch(WEATHER, None),
        ],
        &always("weather"),
    );
    *host.ui_answer.borrow_mut() = Some(Box::new(|call: &UiCall| match call {
        UiCall::View(_) => Ok(String::from("w2: [t1] textbox \"check the weather\"")),
        _ => Ok(String::new()),
    }));
    let reply = shell.eval("look at notes and get the weather in Leeds");
    asked(&host, &reply);
    assert!(
        reply
            .text
            .contains("what a window shows chose this read from a site allowed for an app"),
        "{reply:?}"
    );
    assert!(!reply.text.contains("send"), "nothing is sent: {reply:?}");
    assert!(!reply.to_wire().contains("Always"), "{reply:?}");
}

#[test]
fn a_notes_text_to_a_granted_host_asks() {
    let code = format!("let t = fs_read(\"notes/todo.txt\");\nnet_fetch(\"{WEATHER}\", t);");
    let script = serde_json::json!({"name": "script", "arguments": {"code": code}});
    let (host, mut shell) = granted(
        vec![Ok(llmproto::act::parse(&script.to_string()).unwrap())],
        &always("weather"),
    );
    host.files
        .borrow_mut()
        .insert("notes/todo.txt".into(), "buy milk".into());
    asked(&host, &shell.eval(LINE));
}

#[test]
fn a_fetch_a_note_decided_asks() {
    let code = format!("if fs_read(\"notes/todo.txt\") == \"\" {{ net_fetch(\"{WEATHER}\"); }}");
    let script = serde_json::json!({"name": "script", "arguments": {"code": code}});
    let (host, mut shell) = granted(
        vec![Ok(llmproto::act::parse(&script.to_string()).unwrap())],
        &always("weather"),
    );
    host.files
        .borrow_mut()
        .insert("notes/todo.txt".into(), String::new());
    let reply = shell.eval(LINE);
    asked(&host, &reply);
    assert!(reply.text.contains("chose this read"), "{reply:?}");
}
