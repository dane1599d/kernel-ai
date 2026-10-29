//! The Settings app's requests (P4 task 7): only Settings asks, a change needs
//! a real key or click and is logged as one, with undo, and every argument is
//! read as hostile.

use super::*;

fn host() -> Rc<FakeHost> {
    let host = FakeHost::new();
    host.lists_services.set(true);
    *host.services.borrow_mut() = alloc::vec![
        (String::from("notes"), true),
        (String::from("desktop"), true),
        (String::from("files"), false),
    ];
    host
}

#[test]
fn only_settings_asks_and_a_change_needs_a_click() {
    let host = host();
    let mut shell = shell(&host);
    assert!(shell.settings("notes", "profiles", true).is_err());
    let err = shell
        .settings("settings", "keep-running on notes", false)
        .unwrap_err();
    assert!(err.contains("key or a click"), "{err}");
    assert!(host.kept.borrow().is_empty());
}

#[test]
fn the_grammar_is_exact_and_arguments_are_hostile() {
    let host = host();
    let mut shell = shell(&host);
    for bad in [
        "profiles now",
        "rules-remove -1",
        "rules-remove 1x",
        "rules-remove S",
        "keep-running maybe notes",
        "keep-running on ../notes",
        "keep-running on Notes",
        "profile-set gaming\u{0}",
        "profile-set ",
        "PROFILES",
        "file ../x",
        "delete-everything",
    ] {
        assert!(shell.settings("settings", bad, true).is_err(), "{bad:?}");
    }
    let long = format!("profile-set {}", "a".repeat(600));
    assert!(shell.settings("settings", &long, true).is_err());
    assert!(host.kept.borrow().is_empty());
}

#[test]
fn reads_answer_in_the_apps_format() {
    let host = host();
    let mut shell = shell(&host);
    let profiles = shell.settings("settings", "profiles", false).unwrap();
    assert!(profiles.lines().any(|l| l == "* default"), "{profiles}");
    assert_eq!(
        shell.settings("settings", "rules", false),
        Ok(String::new())
    );
    let about = shell.settings("settings", "about", false).unwrap();
    assert!(about.contains("licence: MPL-2.0"), "{about}");
    assert_eq!(
        shell.settings("settings", "apps", false),
        Ok(String::from("notes\nfiles"))
    );
}

#[test]
fn a_profile_set_by_click_is_logged_as_the_click_and_undoes() {
    let host = host();
    let mut shell = shell(&host);
    assert!(
        shell
            .settings("settings", "profile-set quiet", true)
            .is_ok()
    );
    let profiles = shell.settings("settings", "profiles", false).unwrap();
    assert!(profiles.lines().any(|l| l == "* quiet"), "{profiles}");
    assert!(
        shell.log.text().contains("user-click:settings"),
        "{}",
        shell.log.text()
    );
    assert!(shell.eval("undo").ok);
    let profiles = shell.settings("settings", "profiles", false).unwrap();
    assert!(profiles.lines().any(|l| l == "* default"), "{profiles}");
}

#[test]
fn keep_running_tells_the_supervisor_saves_the_list_and_undoes() {
    let host = host();
    let mut shell = shell(&host);
    assert_eq!(
        shell.settings("settings", "keep-running on notes", true),
        Ok(String::from("notes keep-running on"))
    );
    assert_eq!(*host.kept.borrow(), [(String::from("notes"), true)]);
    assert_eq!(
        host.keep_file.borrow().as_deref(),
        Some("kai-keep 1\nnotes\n")
    );
    // A system service is no app to keep.
    assert!(
        shell
            .settings("settings", "keep-running on desktop", true)
            .is_err()
    );
    assert!(shell.eval("undo").text.starts_with("Undone: "));
    assert_eq!(
        host.kept.borrow().last(),
        Some(&(String::from("notes"), false))
    );
    assert_eq!(host.keep_file.borrow().as_deref(), Some("kai-keep 1\n"));
}

#[test]
fn the_kept_apps_are_told_to_the_supervisor_at_boot() {
    let host = host();
    *host.keep_file.borrow_mut() = Some(String::from("kai-keep 1\nnotes\n"));
    let _shell = shell(&host);
    assert_eq!(*host.kept.borrow(), [(String::from("notes"), true)]);
    let bad = host_with_keep("kai-keep 1\nNot An App\n");
    let _shell = shell(&bad);
    assert!(bad.kept.borrow().is_empty());
}

fn host_with_keep(text: &str) -> Rc<FakeHost> {
    let host = host();
    *host.keep_file.borrow_mut() = Some(String::from(text));
    host
}
