//! The #961 security review's attacks, through the one door: with the weather
//! app's grant on file and the line "what's the weather in Leeds?", none of
//! these fetches anything without a card. And what the shell keeps about
//! grants: an uninstall drops them, a failed boot drop counts only `always`
//! ones, and pkg's answer and its audit are kept, not asked every pass.

use super::appgrants::{LINE, WEATHER, always, asked, file, granted};
use super::*;
use alloc::vec;
use kaigrants::Life;

fn script(code: &str) -> Result<Call, ActError> {
    let call = serde_json::json!({"name": "script", "arguments": {"code": code}});
    Ok(llmproto::act::parse(&call.to_string()).unwrap())
}

/// `code`, with `HOST` the weather host, asks for `LINE` and fetches nothing.
fn script_asks(code: &str) -> Reply {
    let code = code.replace("HOST", &format!("\"{WEATHER}\""));
    let (host, mut shell) = granted(vec![script(&code)], &always("weather"));
    host.files
        .borrow_mut()
        .insert("diary-private.txt".into(), "x".into());
    let reply = shell.eval(LINE);
    asked(&host, &reply);
    reply
}

#[test]
fn a_path_the_script_walk_cannot_read_asks() {
    for code in [
        "net_fetch(HOST, `/q=${\"secret-word\"}`);",
        "net_fetch(HOST, \"/\" + fs_changes());",
        "let c = fs_changes(); net_fetch(HOST, `/${c}`);",
    ] {
        let reply = script_asks(code);
        assert!(
            reply.text.contains("or what to read from it"),
            "{code}: {reply:?}"
        );
    }
}

#[test]
fn a_fetch_the_systems_state_decided_asks() {
    for code in [
        "if fs_changes() == \"\" { net_fetch(HOST, \"/leeds\"); }",
        "for c in fs_changes() { net_fetch(HOST, \"/leeds\"); }",
    ] {
        let reply = script_asks(code);
        assert!(
            reply
                .text
                .contains("the system's own state chose this read"),
            "{code}: {reply:?}"
        );
    }
}

#[test]
fn a_grant_read_in_a_loop_or_twice_asks() {
    for code in [
        "for i in 0..5 { net_fetch(HOST, \"/leeds\"); }",
        "net_fetch(HOST, \"/leeds\"); net_fetch(HOST, \"/in\");",
    ] {
        let reply = script_asks(code);
        assert!(reply.text.contains("more than once"), "{code}: {reply:?}");
    }
}

#[test]
fn a_path_of_punctuation_case_or_repeats_asks() {
    for code in [
        "net_fetch(HOST, \"/-.-.~--.-_..-!.-.$'()*+,;=:@\");",
        "net_fetch(HOST, \"/LeEdS/in/IN/the/WEATHER/in/leeds/leeds/in?the=IN&in=the\");",
    ] {
        script_asks(code);
    }
}

#[test]
fn a_line_that_asks_for_nothing_online_fetches_nothing() {
    let (host, mut shell) = granted(
        vec![super::appgrants::fetch(WEATHER, Some("/?pin=4471"))],
        &always("weather"),
    );
    asked(&host, &shell.eval("my pin is 4471, add it to my notes"));
}

#[test]
fn an_offline_line_says_so_and_offers_no_always() {
    for line in [
        "without looking it up, what's the weather in Leeds?",
        "stay offline and guess the weather in Leeds",
    ] {
        let (host, mut shell) = granted(
            vec![super::appgrants::fetch(WEATHER, None)],
            &always("weather"),
        );
        let reply = shell.eval(line);
        asked(&host, &reply);
        assert!(
            reply.text.contains("said not to go online"),
            "{line}: {reply:?}"
        );
        assert!(!reply.to_wire().contains("Always"), "{line}: {reply:?}");
    }
}

#[test]
fn a_grant_grounds_its_one_host_in_the_shell() {
    for other in ["x.api.weather.example", "weather.example"] {
        let (host, mut shell) = granted(
            vec![super::appgrants::fetch(other, None)],
            &always("weather"),
        );
        asked(&host, &shell.eval(LINE));
    }
}

#[test]
fn an_uninstalled_apps_grants_are_dropped_and_a_new_one_starts_with_none() {
    let host = FakeHost::new();
    host.installed
        .borrow_mut()
        .push(("weather".into(), "kai-actions 1".into()));
    *host.net_disk.borrow_mut() = Some(file(&[
        ("weather", WEATHER, Life::Persistent),
        ("foo-window", "foo.example", Life::Persistent),
    ]));
    assert_eq!(crate::netask::drop_uninstalled(host.as_ref()), 1);
    let left = crate::netgrant::NetGrants::load(&host.net_disk.borrow().clone().unwrap()).unwrap();
    assert_eq!(left.all().len(), 1);
    assert_eq!(left.all()[0].app, "weather");
    // A pkg that gives no clear answer drops nothing.
    *host.net_disk.borrow_mut() = Some(file(&[("foo", "foo.example", Life::Persistent)]));
    *host.installed_down.borrow_mut() = Some("pkg did not answer".into());
    assert_eq!(crate::netask::drop_uninstalled(host.as_ref()), 0);
}

#[test]
fn a_failed_boot_drop_counts_only_always_grants() {
    use nethosts::Granted;
    let host = FakeHost::new();
    host.installed
        .borrow_mut()
        .push(("weather".into(), "kai-actions 1".into()));
    *host.net_disk.borrow_mut() = Some(file(&[("weather", WEATHER, Life::Session)]));
    let memo = crate::appgrants::Memo::default();
    let weather = nethosts::Host::parse(WEATHER).unwrap();
    assert!(crate::appgrants::AppGrants::new(host.as_ref(), &memo).granted(&weather));
    memo.session_stale.set(true);
    assert!(!crate::appgrants::AppGrants::new(host.as_ref(), &memo).granted(&weather));
}

#[test]
fn pkgs_answer_and_its_audit_are_kept_between_passes() {
    use nethosts::Granted;
    let host = FakeHost::new();
    *host.net_disk.borrow_mut() = Some(file(&always("weather")));
    *host.installed_down.borrow_mut() = Some("pkg did not answer".into());
    let memo = crate::appgrants::Memo::default();
    let weather = nethosts::Host::parse(WEATHER).unwrap();
    for _ in 0..3 {
        assert!(!crate::appgrants::AppGrants::new(host.as_ref(), &memo).granted(&weather));
    }
    assert_eq!(host.installed_asked.borrow().len(), 1, "asked once");
    let said = host
        .audit
        .borrow()
        .iter()
        .filter(|a| a.contains("weather's grants count for nothing"))
        .count();
    assert_eq!(said, 1);
    // An install forgets the answer.
    memo.forget_installed();
    let _ = crate::appgrants::AppGrants::new(host.as_ref(), &memo).granted(&weather);
    assert_eq!(host.installed_asked.borrow().len(), 2);
}
