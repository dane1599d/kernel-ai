//! P5 task 13 in the shell: an app's first connection to a new host asks on
//! the terminal's card, naming the app and the host; its Allow writes a
//! per-host grant; Settings lists and revokes them; `undo` takes a grant back
//! and puts a revoked one back.

use super::*;
use crate::NetConnect;
use crate::settings::SETTINGS_APP;
use termcore::menu::{Menu, TAKE_CARD};

/// Weather's package: TCP to 443, names under `weather.example`.
const CEILING: &str = "proto:tcp,addrs:any,ports:443,hosts:weather.example";
const API: &str = "api.weather.example";

fn connect(shell: &mut Shell) -> NetConnect {
    shell.net_connect("weather", API, CEILING)
}

/// The card the terminal takes for the waiting ask.
fn card(shell: &mut Shell) -> Menu {
    assert!(shell.card_waiting());
    let card = shell.eval_console(TAKE_CARD);
    Menu::decode(&card.to_wire()).unwrap_or_else(|e| panic!("{e:?}: {:?}", card.to_wire()))
}

/// Weather asks for `API`, and the answer `word` is given with its ONCE.
fn answered(host: &FakeHost, shell: &mut Shell, word: &str) -> Reply {
    assert_eq!(connect(shell), NetConnect::Asked);
    let menu = card(shell);
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {} {word} once={once}", menu.id))
}

#[test]
fn a_new_host_asks_on_the_card_naming_the_app_and_the_host() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(!shell.card_waiting());
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    // Asked once: a second connection while it waits adds no second card.
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    let menu = card(&mut shell);
    assert_eq!(
        menu.question(),
        "Let weather connect to api.weather.example: net_connect(\"weather\", \"api.weather.example\")?"
    );
    assert_eq!(
        menu.call(),
        "net_connect(\"weather\", \"api.weather.example\")"
    );
    assert!(!shell.card_waiting(), "one card for the one ask");
    // While the card is up, the app is still told it was asked.
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    assert!(!shell.card_waiting());
}

#[test]
fn allow_grants_that_host_alone_and_undo_takes_it_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let done = answered(&host, &mut shell, "allow");
    assert!(done.ok, "{done:?}");
    assert!(
        done.text
            .contains("weather may connect to api.weather.example (this session)")
    );
    let NetConnect::Granted(dest) = connect(&mut shell) else {
        panic!("granted after the Allow");
    };
    assert!(dest.exact && dest.allows_name(API) && !dest.allows_name("www.weather.example"));
    assert!(
        host.net_disk
            .borrow()
            .as_deref()
            .unwrap()
            .contains("object=api.weather.example")
    );
    // Another host under the package's suffix is its own ask.
    assert_eq!(
        shell.net_connect("weather", "maps.weather.example", CEILING),
        NetConnect::Asked
    );
    let undone = shell.eval("undo");
    assert!(
        undone.ok && undone.text.contains("weather may not connect"),
        "{undone:?}"
    );
    assert_eq!(connect(&mut shell), NetConnect::Asked);
}

#[test]
fn a_typed_allow_or_a_deny_grants_nothing() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    let menu = card(&mut shell);
    let typed = shell.eval_console(&format!("answer {} allow", menu.id));
    assert!(typed.text.contains("not run"), "{typed:?}");
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    let denied = answered(&host, &mut shell, "deny");
    assert!(denied.text.contains("not run"), "{denied:?}");
    assert!(host.net_disk.borrow().is_none(), "nothing written");
}

#[test]
fn always_lasts_a_restart_and_allow_does_not() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(answered(&host, &mut shell, "allow").ok);
    assert_eq!(
        shell.net_connect("weather", "maps.weather.example", CEILING),
        NetConnect::Asked
    );
    let menu = card(&mut shell);
    let once = host.mint(menu.id, menu.call());
    let always = shell.eval_console(&format!("answer {} always-target once={once}", menu.id));
    assert!(always.text.contains("(always)"), "{always:?}");
    // A restart drops the session grant and keeps the lasting one.
    let mut again = super::shell(&host);
    assert_eq!(connect(&mut again), NetConnect::Asked);
    assert!(matches!(
        again.net_connect("weather", "maps.weather.example", CEILING),
        NetConnect::Granted(_)
    ));
}

#[test]
fn settings_lists_and_revokes_and_undo_puts_it_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(answered(&host, &mut shell, "allow").ok);
    let listed = shell.settings(SETTINGS_APP, "net", false).unwrap();
    assert_eq!(listed, "#1 weather api.weather.example session");
    // A revoke needs a key or a click.
    assert!(shell.settings(SETTINGS_APP, "net-revoke 1", false).is_err());
    assert!(shell.settings(SETTINGS_APP, "net-revoke 9", true).is_err());
    let revoked = shell.settings(SETTINGS_APP, "net-revoke 1", true).unwrap();
    assert_eq!(revoked, "weather may not connect to api.weather.example");
    assert_eq!(shell.settings(SETTINGS_APP, "net", false).unwrap(), "");
    // The next connection is not granted: it asks again.
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    // `undo` puts the grant back under its own id.
    let undone = shell.eval("undo");
    assert!(undone.ok, "{undone:?}");
    assert!(matches!(connect(&mut shell), NetConnect::Granted(_)));
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#1 weather api.weather.example session"
    );
}

#[test]
fn nothing_outside_the_package_is_asked_and_a_bad_file_fails_closed() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(matches!(
        shell.net_connect("weather", "evil.example", CEILING),
        NetConnect::Refused(_)
    ));
    assert!(matches!(
        shell.net_connect("weather", API, "not a destination"),
        NetConnect::Refused(_)
    ));
    assert!(!shell.card_waiting());
    *host.net_disk.borrow_mut() = Some(String::from("kai-grants 1\nnot a line\n"));
    assert!(
        matches!(connect(&mut shell), NetConnect::Refused(why) if why.contains("out of shape"))
    );
    assert!(!shell.card_waiting());
    assert_eq!(
        host.net_disk.borrow().as_deref(),
        Some("kai-grants 1\nnot a line\n"),
        "left as it is"
    );
}

#[test]
fn the_asks_waiting_are_bounded() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for i in 0..crate::netask::MAX_CARDS {
        let name = format!("h{i}.weather.example");
        assert_eq!(
            shell.net_connect("weather", &name, CEILING),
            NetConnect::Asked
        );
    }
    assert!(matches!(
        shell.net_connect("weather", "last.weather.example", CEILING),
        NetConnect::Refused(_)
    ));
}

#[test]
fn the_wire_says_what_it_came_to() {
    assert_eq!(NetConnect::Asked.wire(), "asked");
    assert_eq!(NetConnect::Refused(String::from("no")).wire(), "refused no");
    let dest =
        netdest::Dest::parse("proto:tcp,addrs:none,ports:443,hosts:a.example,match:exact").unwrap();
    assert_eq!(
        NetConnect::Granted(dest).wire(),
        "granted proto:tcp,addrs:none,ports:443,hosts:a.example,match:exact"
    );
}

/// Kai's `undo` call, answered for the next line it is asked about.
fn kai_undo(host: &FakeHost) {
    let undo = r#"{"name":"undo","arguments":{"steps":1}}"#;
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(undo).unwrap()));
}

#[test]
fn kai_s_own_undo_never_gives_a_grant_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(answered(&host, &mut shell, "allow").ok);
    shell.settings(SETTINGS_APP, "net-revoke 1", true).unwrap();
    // Kai may not undo the revoke (K on #750): the grant stays gone.
    kai_undo(&host);
    let refused = shell.eval("undo that");
    assert!(
        !refused.ok && refused.text.contains("only you may undo"),
        "{refused:?}"
    );
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    // The person's typed `undo` may.
    assert!(shell.eval("undo").ok);
    assert!(matches!(connect(&mut shell), NetConnect::Granted(_)));
    // Taking a grant back stays open to Kai: its undo of that restore
    // revokes again.
    kai_undo(&host);
    let narrowed = shell.eval("undo that");
    assert!(narrowed.ok, "{narrowed:?}");
    assert_eq!(connect(&mut shell), NetConnect::Asked);
}

#[test]
fn kai_s_undo_of_the_person_s_undo_of_an_allow_is_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(answered(&host, &mut shell, "allow").ok);
    assert!(shell.eval("undo").ok, "the person takes the Allow back");
    kai_undo(&host);
    let refused = shell.eval("undo that");
    assert!(!refused.ok, "{refused:?}");
    assert_eq!(connect(&mut shell), NetConnect::Asked);
}
