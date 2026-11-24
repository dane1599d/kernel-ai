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
    shell.save_rules();
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
