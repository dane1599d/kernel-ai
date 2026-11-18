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
            reply.text.contains("It reads from a site"),
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
    let reply = script_asks("for i in 0..5 { net_fetch(HOST, \"/leeds\"); }");
    assert!(reply.text.contains("more than once"), "{reply:?}");
    // A second read met the first one's page: that steers it.
    let reply = script_asks("net_fetch(HOST, \"/leeds\"); net_fetch(HOST, \"/in\");");
    assert!(
        reply.text.contains("a web page chose this read"),
        "{reply:?}"
    );
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
        vec![super::appgrants::fetch(WEATHER, Some("/4471"))],
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
        let pending = shell.pending.as_ref().expect("asks");
        assert!(pending.offer.is_none(), "{line}: no Always offered");
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
fn an_undone_packages_grants_are_dropped_and_no_one_elses() {
    let host = FakeHost::new();
    *host.net_disk.borrow_mut() = Some(file(&[
        ("weather", WEATHER, Life::Persistent),
        ("foo-window", "foo.example", Life::Persistent),
        ("foo", "api.foo.example", Life::Session),
        // A plan-line app pkg never installed keeps its grant.
        ("net-fixture", "a.kai.test", Life::Persistent),
    ]));
    assert_eq!(
        crate::netask::drop_package_grants(host.as_ref(), "foo", "the undo took foo out"),
        Ok(2)
    );
    let left = crate::netgrant::NetGrants::load(&host.net_disk.borrow().clone().unwrap()).unwrap();
    let apps: Vec<&str> = left.all().iter().map(|g| g.app.as_str()).collect();
    assert_eq!(apps, ["weather", "net-fixture"]);
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|a| a.contains("(foo on foo.example) dropped: the undo took foo out")),
        "{:?}",
        host.audit.borrow()
    );
    // A file out of shape is left as it is, and says why.
    *host.net_disk.borrow_mut() = Some(String::from(
        "kai-grants 1
not a line
",
    ));
    assert!(
        crate::netask::drop_package_grants(host.as_ref(), "foo", "the undo took foo out").is_err()
    );
}

#[test]
fn the_package_a_local_install_logged_is_read_back() {
    use crate::netask::local_package;
    assert_eq!(
        local_package(
            "cc build foo 0.0.3
provenance x"
        )
        .as_deref(),
        Some("foo")
    );
    let installed = "app_install(\"foo\")
source /home/user/apps/foo/main.c sha256 ab
artifact /home/user/apps/foo.kai sha256 cd";
    assert_eq!(local_package(installed).as_deref(), Some("foo"));
    assert_eq!(local_package("set_policy(\"rr\")"), None);
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

/// The person's own Always rule for the weather host.
fn always_rule(shell: &mut Shell) {
    let offer = crate::rules::Offer {
        action: "net_fetch",
        target: Some(String::from(WEATHER)),
        kind: false,
    };
    shell.rules.add(&offer, true, None, (0, 0)).unwrap();
}

#[test]
fn the_persons_always_rule_waives_a_grant_ask_but_not_an_offline_line() {
    // As before grants grounded anything: the rule waives the ask for its host.
    let (host, mut shell) = granted(
        vec![super::appgrants::fetch(WEATHER, Some("/?s=x"))],
        &always("weather"),
    );
    always_rule(&mut shell);
    let reply = shell.eval(LINE);
    assert!(reply.ok && reply.confirm.is_none(), "{reply:?}");
    assert_eq!(host.net_seen.borrow().len(), 1);
    // A line that said to stay offline is its own word: no rule waives it.
    let (host, mut shell) = granted(
        vec![super::appgrants::fetch(WEATHER, None)],
        &always("weather"),
    );
    always_rule(&mut shell);
    asked(
        &host,
        &shell.eval("stay offline and guess the weather in Leeds"),
    );
}

#[test]
fn a_fetch_right_after_the_undo_of_its_apps_install_asks() {
    let (host, mut shell) = granted(
        vec![super::appgrants::fetch(WEATHER, None)],
        &always("weather"),
    );
    // The memo holds pkg's "installed" when the undo runs.
    use nethosts::Granted;
    let weather = nethosts::Host::parse(WEATHER).unwrap();
    assert!(crate::appgrants::AppGrants::new(host.as_ref(), &shell.grant_memo).granted(&weather));
    host.installed.borrow_mut().clear();
    shell.local_removed("cc build weather 0.0.2\nprovenance x");
    asked(&host, &shell.eval(LINE));
}

#[test]
fn a_steered_change_is_named_before_a_grant_read() {
    // A change steered by a note, and a read only a grant grounds: the
    // question names the note.
    let code = "if fs_read(\"notes/todo.txt\") == \"\" { fs_empty_trash(); }\nnet_fetch(HOST, \"/leeds\");";
    let reply = script_asks(code);
    assert!(reply.text.contains("the target came from"), "{reply:?}");
}

#[test]
fn a_value_laundered_through_a_method_or_an_abort_still_steers() {
    for code in [
        "let a = []; a.push(fs_changes()); if a[0] == \"x\" { net_fetch(HOST, \"/leeds\"); }",
        "let a = []; a.insert(0, fs_changes()); if a[0] == \"x\" { net_fetch(HOST, \"/leeds\"); }",
        "let a = []; a.append([fs_changes()]); if a[0] == \"x\" { net_fetch(HOST, \"/leeds\"); }",
        "let k = 10 / fs_changes().len(); net_fetch(HOST, \"/leeds\");",
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
fn an_offline_line_asks_before_a_view_steered_read_under_a_rule() {
    use crate::ui::UiCall;
    let (host, mut shell) = granted(
        vec![
            super::appgrants::call(r#"{"name":"ui_view","arguments":{"window":"notes"}}"#),
            super::appgrants::fetch(WEATHER, Some("/leeds")),
        ],
        &always("weather"),
    );
    always_rule(&mut shell);
    *host.ui_answer.borrow_mut() = Some(alloc::boxed::Box::new(|call: &UiCall| match call {
        UiCall::View(_) => Ok(String::from("w2: [t1] textbox \"weather\"")),
        _ => Ok(String::new()),
    }));
    let reply = shell.eval("don't go online, just look at notes and guess the weather in Leeds");
    asked(&host, &reply);
    assert!(
        reply.text.contains("The request said not to go online"),
        "{reply:?}"
    );
    assert!(shell.pending.as_ref().expect("asks").offer.is_none());
}

#[test]
fn a_negation_in_another_sentence_runs_with_or_without_the_rule() {
    let line = "I don't know the weather in Leeds. Look it up online.";
    for rule in [false, true] {
        let (host, mut shell) = granted(
            vec![super::appgrants::fetch(WEATHER, None)],
            &always("weather"),
        );
        if rule {
            always_rule(&mut shell);
        }
        let reply = shell.eval(line);
        assert!(
            reply.ok && reply.confirm.is_none(),
            "rule {rule}: {reply:?}"
        );
        assert_eq!(host.net_seen.borrow().len(), 1);
    }
}

#[test]
fn an_answer_after_any_question_asks_under_the_rule() {
    for line in [
        "Can you look it up online? No, guess the weather in Leeds",
        "Go online, or just guess? Nope. The weather in Leeds",
        "Look it up online, or guess? No. The weather in Leeds",
        "Go online, maybe? No. The weather in Leeds",
    ] {
        let (host, mut shell) = granted(
            vec![super::appgrants::fetch(WEATHER, None)],
            &always("weather"),
        );
        always_rule(&mut shell);
        let reply = shell.eval(line);
        asked(&host, &reply);
        assert!(
            reply.text.contains("said not to go online"),
            "{line}: {reply:?}"
        );
    }
}
