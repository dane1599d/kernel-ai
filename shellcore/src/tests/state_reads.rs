//! A state file the host could not read is never read as none: a grant,
//! rule, instruction or kept app written then would replace the whole file
//! without what it held.

use super::*;
use crate::NetConnect;
use crate::settings::SETTINGS_APP;
use termcore::menu::{Menu, TAKE_CARD};

/// Weather's package: TCP to 443, names under `weather.example`.
const CEILING: &str = "proto:tcp,addrs:any,ports:443,hosts:weather.example";

fn audited(host: &FakeHost, text: &str) -> bool {
    host.audit.borrow().iter().any(|line| line.contains(text))
}

/// Weather asks for `name`: the card the terminal takes for it.
fn asks(shell: &mut Shell, name: &str) -> Menu {
    assert_eq!(
        shell.net_connect("weather", name, CEILING),
        NetConnect::Asked
    );
    let card = shell.eval_console(TAKE_CARD);
    Menu::decode(&card.to_wire()).unwrap_or_else(|e| panic!("{e:?}: {:?}", card.to_wire()))
}

fn allow(host: &FakeHost, shell: &mut Shell, menu: &Menu) -> Reply {
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {} allow once={once}", menu.id))
}

#[test]
fn a_grant_is_never_written_over_a_grants_file_that_could_not_be_read() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let menu = asks(&mut shell, "api.weather.example");
    assert!(allow(&host, &mut shell, &menu).ok);
    let before = host.net_disk.borrow().clone();
    assert!(
        before
            .as_deref()
            .unwrap()
            .contains("object=api.weather.example")
    );
    // The card is up when the disk stops answering: the Allow is refused,
    // the person is told why, and the file keeps the earlier grant.
    let menu = asks(&mut shell, "maps.weather.example");
    host.read_errors.borrow_mut().push("net_grants_read");
    let reply = allow(&host, &mut shell, &menu);
    assert!(!reply.ok, "{reply:?}");
    assert!(
        reply.text.contains(
            "weather was not allowed maps.weather.example: \
             the network grants on disk could not be read"
        ),
        "{reply:?}"
    );
    assert_eq!(*host.net_disk.borrow(), before);
    // While it cannot be read, a connection is refused, never asked.
    assert!(matches!(
        shell.net_connect("weather", "www.weather.example", CEILING),
        NetConnect::Refused(why) if why.contains("could not be read")
    ));
    assert!(!shell.card_waiting());
    host.read_errors.borrow_mut().clear();
    assert!(matches!(
        shell.net_connect("weather", "api.weather.example", CEILING),
        NetConnect::Granted(_)
    ));
}

#[test]
fn rules_that_could_not_be_read_at_start_are_never_written_over() {
    let host = FakeHost::new();
    let saved =
        "kai-grants 1\nrule 1 action=set_policy target=- from=prompt:3 made=1970-01-01T00:00:00Z\n";
    *host.rules_disk.borrow_mut() = Some(String::from(saved));
    host.read_errors.borrow_mut().push("rules_read");
    let shell = shell(&host);
    host.read_errors.borrow_mut().clear();
    assert!(audited(
        &host,
        "the rules on disk could not be read (the disk did not answer); starting with none"
    ));
    // Every rule change saves through here.
    assert!(shell.save_rules().is_err());
    assert_eq!(host.rules_disk.borrow().as_deref(), Some(saved));
    assert!(audited(
        &host,
        "the rules were not written: the rules on disk could not be read at start"
    ));
}

#[test]
fn standing_instructions_that_could_not_be_read_at_start_are_never_written_over() {
    let host = FakeHost::new();
    host.triggers_on.set(true);
    let saved = String::from("kept as it was");
    *host.standing_disk.borrow_mut() = Some(saved.clone());
    host.read_errors.borrow_mut().push("standing_read");
    let mut shell = shell(&host);
    host.read_errors.borrow_mut().clear();
    let rule = "on minimize(notes) -> app_suspend(notes) until restore(notes)";
    assert!(
        shell
            .eval(&format!("rules_add {rule}"))
            .to_wire()
            .starts_with("menu 1 ")
    );
    let once = host.mint(1, &format!("rules_add(\"{rule}\")"));
    let made = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(made.ok, "{made:?}");
    assert_eq!(host.standing_disk.borrow().as_deref(), Some(saved.as_str()));
    assert!(audited(
        &host,
        "the standing instructions were not written: \
         the standing instructions on disk could not be read at start"
    ));
}

#[test]
fn a_keep_running_list_that_could_not_be_read_at_start_is_never_written_over() {
    let host = FakeHost::new();
    host.lists_services.set(true);
    *host.services.borrow_mut() = alloc::vec![(String::from("notes"), true)];
    *host.keep_file.borrow_mut() = Some(String::from("kai-keep 1\nfiles\n"));
    host.read_errors.borrow_mut().push("keep_read");
    let mut shell = shell(&host);
    host.read_errors.borrow_mut().clear();
    assert_eq!(
        shell.settings(SETTINGS_APP, "keep-running on notes", true),
        Ok(String::from("notes keep-running on"))
    );
    assert_eq!(
        host.keep_file.borrow().as_deref(),
        Some("kai-keep 1\nfiles\n")
    );
    assert!(audited(
        &host,
        "the keep-running list was not saved: \
         the keep-running list on disk could not be read at start"
    ));
}

/// Weather, installed, with `api.weather.example` allowed this session and
/// `maps.weather.example` always; then a restart whose read of the grants
/// fails, and the disk answering again after it.
fn restarted_unread() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.installed
        .borrow_mut()
        .push(("weather".into(), "kai-actions 1".into()));
    let mut before = shell(&host);
    let menu = asks(&mut before, "api.weather.example");
    assert!(allow(&host, &mut before, &menu).ok);
    let menu = asks(&mut before, "maps.weather.example");
    let once = host.mint(menu.id, menu.call());
    let always = before.eval_console(&format!("answer {} always-target once={once}", menu.id));
    assert!(always.text.contains("(always)"), "{always:?}");
    host.read_errors.borrow_mut().push("net_grants_read");
    let after = shell(&host);
    host.read_errors.borrow_mut().clear();
    (host, after)
}

fn grounds(host: &FakeHost, shell: &Shell, name: &str) -> bool {
    use nethosts::Granted;
    let name = nethosts::Host::parse(name).unwrap();
    crate::appgrants::AppGrants::new(host, &shell.grant_memo).granted(&name)
}

#[test]
fn last_sessions_grant_grants_nothing_after_a_boot_that_could_not_read_them() {
    let (host, mut shell) = restarted_unread();
    assert!(
        host.net_disk
            .borrow()
            .as_deref()
            .unwrap()
            .contains("object=api.weather.example"),
        "the boot could not drop it"
    );
    assert!(!grounds(&host, &shell, "api.weather.example"));
    assert!(grounds(&host, &shell, "maps.weather.example"));
    assert_eq!(
        shell.net_connect("weather", "api.weather.example", CEILING),
        NetConnect::Asked
    );
}

#[test]
fn the_owed_drop_lands_at_the_next_good_read_and_keeps_the_lasting_grants() {
    let (host, mut shell) = restarted_unread();
    assert!(matches!(
        shell.net_connect("weather", "maps.weather.example", CEILING),
        NetConnect::Granted(_)
    ));
    let disk = host.net_disk.borrow().clone().unwrap();
    assert!(!disk.contains("object=api.weather.example"), "{disk}");
    assert!(disk.contains("object=maps.weather.example"), "{disk}");
    assert!(audited(&host, "last session's network grants were dropped"));
    // Dropped once: the person's Allow now is this session's, and stays.
    let menu = asks(&mut shell, "api.weather.example");
    assert!(allow(&host, &mut shell, &menu).ok);
    assert!(matches!(
        shell.net_connect("weather", "api.weather.example", CEILING),
        NetConnect::Granted(_)
    ));
    assert!(grounds(&host, &shell, "api.weather.example"));
}

#[test]
fn an_allow_whose_write_lands_the_owed_drop_keeps_its_grant() {
    let (host, mut shell) = restarted_unread();
    // The owed drop cannot be written while weather asks and the card is up.
    host.net_write_fails.set(u32::MAX);
    let menu = asks(&mut shell, "api.weather.example");
    // The Allow's drop write fails; the grant's write, of the dropped view
    // and the new grant, lands both.
    host.net_write_fails.set(1);
    let reply = allow(&host, &mut shell, &menu);
    assert!(reply.ok, "{reply:?}");
    assert!(matches!(
        shell.net_connect("weather", "api.weather.example", CEILING),
        NetConnect::Granted(_)
    ));
    let disk = host.net_disk.borrow().clone().unwrap();
    assert_eq!(
        disk.matches("object=api.weather.example").count(),
        1,
        "{disk}"
    );
    assert!(!disk.contains("grant 1 "), "last session's is gone: {disk}");
    assert!(disk.contains("object=maps.weather.example"), "{disk}");
    assert!(!shell.grant_memo.session_stale.get());
}

#[test]
fn settings_lists_only_the_grants_that_grant_while_the_drop_is_owed() {
    let (host, mut shell) = restarted_unread();
    let listed = shell.settings(SETTINGS_APP, "net", false).unwrap();
    assert!(!listed.contains("api.weather.example"), "{listed}");
    assert!(listed.contains("maps.weather.example always"), "{listed}");
    assert!(
        host.net_disk
            .borrow()
            .as_deref()
            .unwrap()
            .contains("object=api.weather.example"),
        "a listing writes nothing"
    );
}

#[test]
fn a_revoke_while_the_drop_is_owed_writes_the_dropped_view() {
    let (host, mut shell) = restarted_unread();
    // The owed drop's own write fails; the revoke's write, of the dropped
    // view without maps, lands both.
    host.net_write_fails.set(1);
    let revoked = shell.settings(SETTINGS_APP, "net-revoke 2", true).unwrap();
    assert!(revoked.contains("maps.weather.example"), "{revoked}");
    let disk = host.net_disk.borrow().clone().unwrap();
    assert!(!disk.contains("object=api.weather.example"), "{disk}");
    assert!(!disk.contains("object=maps.weather.example"), "{disk}");
    assert!(!shell.grant_memo.session_stale.get());
    assert!(audited(&host, "last session's network grants were dropped"));
    // Its undo puts back maps alone, never last session's api.
    let undone = shell.eval_console("undo");
    assert!(undone.ok, "{undone:?}");
    let disk = host.net_disk.borrow().clone().unwrap();
    assert!(disk.contains("object=maps.weather.example"), "{disk}");
    assert!(!disk.contains("object=api.weather.example"), "{disk}");
}

#[test]
fn an_owed_drop_whose_write_fails_twice_still_grants_nothing() {
    let (host, mut shell) = restarted_unread();
    host.net_write_fails.set(2);
    for _ in 0..2 {
        assert_eq!(
            shell.net_connect("weather", "api.weather.example", CEILING),
            NetConnect::Asked
        );
    }
    assert!(audited(
        &host,
        "the session network grants were not dropped: the disk did not answer"
    ));
    assert!(shell.grant_memo.session_stale.get());
    assert!(!grounds(&host, &shell, "api.weather.example"));
    assert!(
        host.net_disk
            .borrow()
            .as_deref()
            .unwrap()
            .contains("object=api.weather.example"),
        "neither write landed"
    );
    // The disk answers: the third try lands it.
    assert!(matches!(
        shell.net_connect("weather", "maps.weather.example", CEILING),
        NetConnect::Granted(_)
    ));
    let disk = host.net_disk.borrow().clone().unwrap();
    assert!(!disk.contains("object=api.weather.example"), "{disk}");
    assert!(!shell.grant_memo.session_stale.get());
}

#[test]
fn a_packages_grants_dropped_while_the_drop_is_owed_write_the_dropped_view() {
    use kaigrants::Life;
    let host = FakeHost::new();
    *host.net_disk.borrow_mut() = Some(super::appgrants::file(&[
        ("weather", "api.weather.example", Life::Session),
        ("foo", "api.foo.example", Life::Persistent),
        ("weather", "maps.weather.example", Life::Persistent),
    ]));
    host.read_errors.borrow_mut().push("net_grants_read");
    let mut shell = shell(&host);
    host.read_errors.borrow_mut().clear();
    assert!(shell.grant_memo.session_stale.get());
    // The owed drop's own write fails; the undo of foo's install drops
    // foo's grant, and its write, of the dropped view, lands both.
    host.net_write_fails.set(1);
    shell.local_removed("cc build foo 0.0.3\nprovenance x");
    let disk = host.net_disk.borrow().clone().unwrap();
    assert!(!disk.contains("object=api.foo.example"), "{disk}");
    assert!(!disk.contains("object=api.weather.example"), "{disk}");
    assert!(disk.contains("object=maps.weather.example"), "{disk}");
    assert!(!shell.grant_memo.session_stale.get());
    assert!(audited(
        &host,
        "(foo on api.foo.example) dropped: the undo took foo out"
    ));
    assert!(audited(&host, "last session's network grants were dropped"));
}

#[test]
fn a_change_whose_own_write_fails_too_leaves_the_drop_owed() {
    let (host, mut shell) = restarted_unread();
    // The owed drop's write fails, and so does the revoke's own.
    host.net_write_fails.set(2);
    let why = shell
        .settings(SETTINGS_APP, "net-revoke 2", true)
        .unwrap_err();
    assert!(why.contains("the disk did not answer"), "{why}");
    assert!(
        shell.grant_memo.session_stale.get(),
        "the drop is still owed"
    );
    let disk = host.net_disk.borrow().clone().unwrap();
    assert!(disk.contains("object=api.weather.example"), "{disk}");
    assert!(disk.contains("object=maps.weather.example"), "{disk}");
    // Last session's grant, still on disk, grants nothing.
    assert!(!grounds(&host, &shell, "api.weather.example"));
    assert_eq!(
        shell.net_connect("weather", "api.weather.example", CEILING),
        NetConnect::Asked
    );
}
