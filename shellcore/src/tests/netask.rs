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
    // The connection that asked is refused, naming the host, and so is the
    // next, with no card, until the Deny lapses; then it asks again.
    for _ in 0..2 {
        assert!(matches!(
            connect(&mut shell),
            NetConnect::Refused(why) if why.contains("did not allow weather to connect to api.weather.example")
        ));
        assert!(!shell.card_waiting());
    }
    host.clock
        .set(host.clock.get() + crate::netask::DENY_HOLDS_NS);
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    // A typed Allow is no Deny: nothing is refused for it.
    let menu = card(&mut shell);
    shell.eval_console(&format!("answer {} allow", menu.id));
    assert_eq!(connect(&mut shell), NetConnect::Asked);
}

#[test]
fn a_grant_put_back_by_undo_comes_before_a_deny() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(answered(&host, &mut shell, "allow").ok);
    shell.settings(SETTINGS_APP, "net-revoke 1", true).unwrap();
    // Asked again after the revoke, and denied.
    let denied = answered(&host, &mut shell, "deny");
    assert!(denied.text.contains("not run"), "{denied:?}");
    assert!(matches!(connect(&mut shell), NetConnect::Refused(_)));
    // The person's `undo` puts the revoked grant back: granted at once.
    assert!(shell.eval("undo").ok);
    assert!(matches!(connect(&mut shell), NetConnect::Granted(_)));
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

/// Maps' package: TCP to 443, names under `maps.example`.
const MAPS: &str = "proto:tcp,addrs:any,ports:443,hosts:maps.example";
const TILES: &str = "tiles.maps.example";

/// `app` asks for `name` under `ceiling`, and the person clicks Allow.
fn allowed(host: &FakeHost, shell: &mut Shell, app: &str, name: &str, ceiling: &str) {
    assert_eq!(shell.net_connect(app, name, ceiling), NetConnect::Asked);
    let menu = card(shell);
    let once = host.mint(menu.id, menu.call());
    let done = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(done.ok, "{done:?}");
}

#[test]
fn undo_of_a_revoke_puts_back_only_the_app_s_host_it_took() {
    // Issue #830, on an empty grants file.
    let host = FakeHost::new();
    let mut shell = shell(&host);
    // 1. Weather asks for its host, Allow: grant #1. 2. Revoked in Settings.
    allowed(&host, &mut shell, "weather", API, CEILING);
    shell.settings(SETTINGS_APP, "net-revoke 1", true).unwrap();
    // 3. Maps asks for its host, Allow: a fresh id, never #1 again.
    allowed(&host, &mut shell, "maps", TILES, MAPS);
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#2 maps tiles.maps.example session"
    );
    // 4. Revoked in Settings.
    assert!(shell.settings(SETTINGS_APP, "net-revoke 1", true).is_err());
    shell.settings(SETTINGS_APP, "net-revoke 2", true).unwrap();
    // 5. `undo` puts back maps' host alone; weather's stays revoked.
    let undone = shell.eval("undo");
    assert!(
        undone.ok
            && undone
                .text
                .contains("maps may connect to tiles.maps.example"),
        "{undone:?}"
    );
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#2 maps tiles.maps.example session"
    );
    assert!(matches!(
        shell.net_connect("maps", TILES, MAPS),
        NetConnect::Granted(_)
    ));
    assert_eq!(connect(&mut shell), NetConnect::Asked);
}

#[test]
fn a_reload_never_lowers_the_next_grant_id() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    allowed(&host, &mut shell, "weather", API, CEILING);
    allowed(
        &host,
        &mut shell,
        "weather",
        "maps.weather.example",
        CEILING,
    );
    // The newest grant goes: the file's highest id is #1 again.
    shell.settings(SETTINGS_APP, "net-revoke 2", true).unwrap();
    allowed(&host, &mut shell, "maps", TILES, MAPS);
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#1 weather api.weather.example session\n#3 maps tiles.maps.example session"
    );
}

#[test]
fn undo_of_an_allow_refuses_a_grant_now_for_another_host() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    allowed(&host, &mut shell, "weather", API, CEILING);
    // Grant #1 on disk now names another host than the Allow logged.
    let moved = host
        .net_disk
        .borrow()
        .as_deref()
        .unwrap()
        .replace("object=api.weather.example", "object=www.weather.example");
    *host.net_disk.borrow_mut() = Some(moved.clone());
    let undone = shell.eval("undo");
    assert!(!undone.ok, "{undone:?}");
    assert_eq!(host.net_disk.borrow().as_deref(), Some(moved.as_str()));
}

#[test]
fn a_restart_numbers_new_grants_above_the_session_grants_it_dropped() {
    let host = FakeHost::new();
    let mut first = shell(&host);
    allowed(&host, &mut first, "weather", API, CEILING);
    allowed(
        &host,
        &mut first,
        "weather",
        "maps.weather.example",
        CEILING,
    );
    // A restart drops both session grants: the file holds no id at all.
    let mut again = super::shell(&host);
    assert_eq!(again.settings(SETTINGS_APP, "net", false).unwrap(), "");
    // The mark was seeded at boot before the drop: #1 and #2 are not given again.
    allowed(&host, &mut again, "maps", TILES, MAPS);
    assert_eq!(
        again.settings(SETTINGS_APP, "net", false).unwrap(),
        "#3 maps tiles.maps.example session"
    );
}

#[test]
fn undo_then_redo_of_a_revoke_takes_the_same_grant_again() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    allowed(&host, &mut shell, "weather", API, CEILING);
    allowed(&host, &mut shell, "maps", TILES, MAPS);
    shell.settings(SETTINGS_APP, "net-revoke 1", true).unwrap();
    assert!(shell.eval("undo").ok, "the revoke is undone");
    assert!(matches!(connect(&mut shell), NetConnect::Granted(_)));
    // `undo` again redoes the revoke: weather's #1 goes, maps' #2 stays.
    let redone = shell.eval("undo");
    assert!(
        redone.ok
            && redone
                .text
                .contains("weather may not connect to api.weather.example"),
        "{redone:?}"
    );
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#2 maps tiles.maps.example session"
    );
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    // And again puts weather's back under its own id (listed after maps').
    let back = shell.eval("undo");
    assert!(back.ok, "{back:?}");
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#2 maps tiles.maps.example session\n#1 weather api.weather.example session"
    );
}

#[test]
fn redo_of_a_revoke_refuses_a_grant_now_for_another_host() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    allowed(&host, &mut shell, "weather", API, CEILING);
    shell.settings(SETTINGS_APP, "net-revoke 1", true).unwrap();
    assert!(shell.eval("undo").ok, "the revoke is undone");
    // Grant #1 on disk now names another host than the revoke logged.
    let moved = host
        .net_disk
        .borrow()
        .as_deref()
        .unwrap()
        .replace("object=api.weather.example", "object=www.weather.example");
    *host.net_disk.borrow_mut() = Some(moved.clone());
    let redone = shell.eval("undo");
    assert!(!redone.ok, "{redone:?}");
    assert_eq!(host.net_disk.borrow().as_deref(), Some(moved.as_str()));
}

#[test]
fn redo_of_an_allow_s_undo_puts_the_grant_back_under_its_id() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    allowed(&host, &mut shell, "weather", API, CEILING);
    assert!(shell.eval("undo").ok, "the person takes the Allow back");
    assert_eq!(connect(&mut shell), NetConnect::Asked);
    // `undo` again redoes the Allow: the same grant, #1, for the same host.
    let redone = shell.eval("undo");
    assert!(
        redone.ok
            && redone
                .text
                .contains("weather may connect to api.weather.example again"),
        "{redone:?}"
    );
    assert!(matches!(connect(&mut shell), NetConnect::Granted(_)));
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#1 weather api.weather.example session"
    );
    // The next Allow is numbered above it.
    allowed(&host, &mut shell, "maps", TILES, MAPS);
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#1 weather api.weather.example session\n#2 maps tiles.maps.example session"
    );
}

#[test]
fn a_put_back_picks_the_kept_grant_by_its_app_and_host_not_its_id_alone() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    allowed(&host, &mut shell, "weather", API, CEILING);
    shell.settings(SETTINGS_APP, "net-revoke 1", true).unwrap();
    // Kept after weather's #1: another app's host under the same id, newest.
    let mut other = shell.net_revoked[0].clone();
    other.app = String::from("maps");
    other.object = String::from(TILES);
    shell.net_revoked.push(other);
    // Neither app and host: refused, both still kept.
    assert!(
        shell
            .revert_net_grant(1, false, "mail's network to mail.example")
            .is_err()
    );
    assert_eq!(shell.net_revoked.len(), 2);
    // Weather's, though maps' is the newest kept under #1.
    let back = shell
        .revert_net_grant(1, false, "weather's network to api.weather.example")
        .unwrap();
    assert_eq!(back, "weather may connect to api.weather.example again");
    assert_eq!(
        shell.settings(SETTINGS_APP, "net", false).unwrap(),
        "#1 weather api.weather.example session"
    );
    assert!(matches!(connect(&mut shell), NetConnect::Granted(_)));
    // Maps' is still kept, and nothing of it reached the file.
    assert_eq!(shell.net_revoked.len(), 1);
    assert_eq!(
        (
            shell.net_revoked[0].app.as_str(),
            shell.net_revoked[0].object.as_str()
        ),
        ("maps", TILES)
    );
    assert_eq!(shell.net_connect("maps", TILES, MAPS), NetConnect::Asked);
}
