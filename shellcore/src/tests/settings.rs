//! The Settings app's requests (P4 task 7): only Settings asks, a change needs
//! a real key or click and is logged as one, with undo, and every argument is
//! read as hostile.

use super::*;
use crate::settings::SETTINGS_APP;

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
        .settings(SETTINGS_APP, "keep-running on notes", false)
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
        assert!(shell.settings(SETTINGS_APP, bad, true).is_err(), "{bad:?}");
    }
    let long = format!("profile-set {}", "a".repeat(600));
    assert!(shell.settings(SETTINGS_APP, &long, true).is_err());
    assert!(host.kept.borrow().is_empty());
}

#[test]
fn reads_answer_in_the_apps_format() {
    let host = host();
    let mut shell = shell(&host);
    let profiles = shell.settings(SETTINGS_APP, "profiles", false).unwrap();
    assert!(profiles.lines().any(|l| l == "* default"), "{profiles}");
    assert_eq!(
        shell.settings(SETTINGS_APP, "rules", false),
        Ok(String::new())
    );
    let about = shell.settings(SETTINGS_APP, "about", false).unwrap();
    assert!(about.contains("licence: MPL-2.0"), "{about}");
    assert_eq!(
        shell.settings(SETTINGS_APP, "apps", false),
        Ok(String::from("notes\nfiles"))
    );
}

#[test]
fn a_profile_set_by_click_is_logged_as_the_click_and_undoes() {
    let host = host();
    let mut shell = shell(&host);
    assert!(
        shell
            .settings(SETTINGS_APP, "profile-set quiet", true)
            .is_ok()
    );
    let profiles = shell.settings(SETTINGS_APP, "profiles", false).unwrap();
    assert!(profiles.lines().any(|l| l == "* quiet"), "{profiles}");
    assert!(
        shell.log.text().contains("user-click:settings"),
        "{}",
        shell.log.text()
    );
    assert!(shell.eval("undo").ok);
    let profiles = shell.settings(SETTINGS_APP, "profiles", false).unwrap();
    assert!(profiles.lines().any(|l| l == "* default"), "{profiles}");
}

#[test]
fn keep_running_tells_the_supervisor_saves_the_list_and_undoes() {
    let host = host();
    let mut shell = shell(&host);
    assert_eq!(
        shell.settings(SETTINGS_APP, "keep-running on notes", true),
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
            .settings(SETTINGS_APP, "keep-running on desktop", true)
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

#[test]
fn a_saved_kept_app_not_serving_yet_is_tried_again_then_given_up() {
    let host = host_with_keep("kai-keep 1\nnotes\n");
    host.keep_fails.set(2);
    let mut first = shell(&host);
    // Refused once at boot, once more on the first retry, then taken.
    first.retry_keep();
    assert!(host.kept.borrow().is_empty(), "not due yet");
    host.clock.set(host.clock.get() + 1_000_000_000);
    first.retry_keep();
    host.clock.set(host.clock.get() + 1_000_000_000);
    first.retry_keep();
    assert_eq!(*host.kept.borrow(), [(String::from("notes"), true)]);
    assert_eq!(
        first.settings(crate::settings::SETTINGS_APP, "keep-running on notes", true),
        Ok(String::from("notes keep-running is on already"))
    );
    // One the supervisor never takes is audited once the retries run out.
    let host = host_with_keep("kai-keep 1\nnotes\n");
    host.keep_fails.set(u32::MAX);
    let mut shell = shell(&host);
    host.clock
        .set(host.clock.get() + crate::settings::KEEP_RETRY_FOR_NS + 1);
    shell.retry_keep();
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|line| line.starts_with("notes is not kept running")),
        "{:?}",
        host.audit.borrow()
    );
    host.clock.set(host.clock.get() + 1_000_000_000);
    shell.retry_keep();
    assert!(host.kept.borrow().is_empty());
}

fn host_with_keep(text: &str) -> Rc<FakeHost> {
    let host = host();
    *host.keep_file.borrow_mut() = Some(String::from(text));
    host
}

/// A `files` app whose effects may call three file rows.
const FILES: &str = "version 1
kai-actions 1
world app:files/files@1.0.0
effects fs_list fs_read fs_move
action files_open
export app:files/actions@1.0.0#open
doc Open a folder.
sig func(path: string)
arg path text max=255 role=target from-request
facts read-only=yes destructive=no idempotent=yes open-world=no
inverse none
end
";

#[test]
fn settings_lists_each_declared_row_and_a_click_withholds_one_with_undo() {
    let host = host();
    host.apps.borrow_mut().rows.load("files", FILES).unwrap();
    let mut shell = shell(&host);
    let app = crate::settings::SETTINGS_APP;
    let all = "files\tfs_list\tallowed\nfiles\tfs_read\tallowed\nfiles\tfs_move\tallowed";
    assert_eq!(shell.settings(app, "grants", false).as_deref(), Ok(all));
    // A change needs the user's own click.
    assert!(
        shell
            .settings(app, "grants-revoke files fs_list", false)
            .is_err()
    );
    assert_eq!(
        shell
            .settings(app, "grants-revoke files fs_list", true)
            .as_deref(),
        Ok("files fs_list withheld")
    );
    let listed = shell.settings(app, "grants files", false).unwrap();
    assert!(listed.starts_with("files\tfs_list\twithheld\n"), "{listed}");
    assert_eq!(
        host.withheld_disk.borrow().as_deref(),
        Some("kai-withheld 1\nfiles fs_list\n")
    );
    // Only a row the app declares; a row given back is the user's click too.
    assert!(
        shell
            .settings(app, "grants-revoke files set_policy", true)
            .is_err()
    );
    assert!(shell.eval("undo").text.starts_with("Undone: "));
    assert_eq!(shell.settings(app, "grants", false).as_deref(), Ok(all));
    shell
        .settings(app, "grants-revoke files fs_read", true)
        .unwrap();
    assert_eq!(
        shell
            .settings(app, "grants-restore files fs_read", true)
            .as_deref(),
        Ok("files fs_read allowed")
    );
    assert_eq!(
        host.withheld_disk.borrow().as_deref(),
        Some("kai-withheld 1\n")
    );
}

#[test]
fn an_odd_name_in_a_grant_change_changes_and_logs_nothing() {
    let host = host();
    host.apps.borrow_mut().rows.load("files", FILES).unwrap();
    *host.withheld_disk.borrow_mut() = Some(String::from("kai-withheld 1\nfiles fs_list\n"));
    let mut shell = shell(&host);
    let app = crate::settings::SETTINGS_APP;
    let next = shell.next_id;
    for bad in [
        "grants-restore files Fs_List",
        "grants-restore files fs_list\"x",
        "grants-revoke Files fs_read",
        "grants-restore files a_row_name_far_too_long_to_hold",
    ] {
        assert!(shell.settings(app, bad, true).is_err(), "{bad:?}");
    }
    assert_eq!(shell.next_id, next, "nothing was logged");
    assert_eq!(
        host.withheld_disk.borrow().as_deref(),
        Some("kai-withheld 1\nfiles fs_list\n")
    );
}
