use super::*;

const NOTES: &str = "version 2
kai-actions 1
world app:notes/notes@1.0.0
action notes_add
export app:notes/actions@1.0.0#add
doc Add a note and return its id.
sig func(text: string) -> result<u32, string>
arg text text max=2000 role=payload from-request
facts read-only=no destructive=no idempotent=no open-world=no
inverse snapshot
end
action notes_count
export app:notes/actions@1.0.0#count
doc How many notes there are.
sig func() -> u32
facts read-only=yes destructive=no idempotent=yes open-world=no
inverse none
end
";

#[test]
fn an_apps_rows_load_as_its_own_with_the_oss_tiers() {
    let mut apps = AppRows::default();
    assert_eq!(apps.load("notes", NOTES), Ok(2));
    let add = apps.find("notes_add").unwrap();
    assert_eq!(add.row.owner, Owner::App("notes"));
    assert_eq!(add.export, "app:notes/actions@1.0.0#add");
    assert_eq!(agentabi::derive(add.row), Tier::Run);
    assert!(matches!(add.row.inverse, Inverse::Snapshot));
    let count = apps.find("notes_count").unwrap();
    assert!(count.row.facts.read_only);
    // The same version again loads nothing new.
    assert_eq!(apps.load("notes", NOTES), Ok(2));
    assert_eq!(apps.rows().count(), 2);
}

#[test]
fn a_fact_left_out_is_pessimistic_and_no_undo_asks() {
    let rows = NOTES.replace(
        "facts read-only=no destructive=no idempotent=no open-world=no\ninverse snapshot",
        "inverse none",
    );
    let mut apps = AppRows::default();
    apps.load("notes", &rows).unwrap();
    let add = apps.find("notes_add").unwrap();
    assert!(add.row.facts.destructive && add.row.facts.open_world);
    assert_eq!(agentabi::derive(add.row), Tier::Ask);
}

#[test]
fn a_window_owns_its_packages_rows_and_only_one_service_does() {
    // Notes' window declares Notes' own rows, `notes_*`.
    let mut apps = AppRows::default();
    assert_eq!(apps.load("notes-window", NOTES), Ok(2));
    let add = apps.find_in("notes-window", "notes_add").unwrap();
    assert_eq!(add.app, "notes-window");
    // Then `notes` cannot load them too, nor the window after `notes`.
    let err = apps.load("notes", NOTES).unwrap_err();
    assert!(err.contains("loaded for its window notes-window"), "{err}");
    let mut apps = AppRows::default();
    apps.load("notes", NOTES).unwrap();
    let err = apps.load("notes-window", NOTES).unwrap_err();
    assert!(err.contains("notes has them loaded"), "{err}");
}

#[test]
fn a_window_fronts_only_the_package_its_world_names() {
    let mut apps = AppRows::default();
    // A window named for notes whose package is another's takes nothing.
    let other = NOTES.replace("app:notes/", "app:files/");
    let err = apps.load("notes-window", &other).unwrap_err();
    assert!(err.contains("does not start notes_window_"), "{err}");
    // A package's world cannot make a service another's window.
    let err = apps.load("files-window", NOTES).unwrap_err();
    assert!(err.contains("does not start files_window_"), "{err}");
    assert_eq!(
        fronts("notes-window", "app:notes/notes@1.0.0"),
        Some("notes")
    );
    assert_eq!(fronts("notes", "app:notes/notes@1.0.0"), None);
    assert_eq!(fronts("notes-window", "app:notes-x/n@1"), None);
    // No name, or one no package could have, fronts nothing.
    assert_eq!(fronts("-window", "app:/x"), None);
    assert_eq!(fronts("Notes-window", "app:Notes/x"), None);
    assert_eq!(fronts("no_tes-window", "app:no_tes/x"), None);
}

#[test]
fn a_window_packed_under_its_own_world_fronts_only_its_packages_exports() {
    // xtask packs Notes' window as `app:notes-window/notes-window@1.0.0`.
    let packed = NOTES.replace(
        "world app:notes/notes@1.0.0",
        "world app:notes-window/notes-window@1.0.0",
    );
    assert_eq!(
        fronts("notes-window", "app:notes-window/notes-window@1.0.0"),
        Some("notes")
    );
    let mut apps = AppRows::default();
    assert_eq!(apps.load("notes-window", &packed), Ok(2));
    assert!(apps.find_in("notes-window", "notes_add").is_some());
    // A `notes_*` row must run Notes' own export, not another package's.
    let other = packed.replace(
        "export app:notes/actions@1.0.0#add",
        "export app:files/actions@1.0.0#trash",
    );
    let err = AppRows::default().load("notes-window", &other).unwrap_err();
    assert!(
        err.contains("runs app:files/actions@1.0.0#trash, not the notes package's"),
        "{err}"
    );
}

#[test]
fn no_package_may_declare_effects_that_type_shell_lines() {
    assert!(!declarable("shell_line"));
    let greedy = NOTES.replace(
        "world app:notes/notes@1.0.0\n",
        "world app:notes/notes@1.0.0\neffects shell_line\n",
    );
    let err = AppRows::default().load("notes", &greedy).unwrap_err();
    assert!(err.contains("shell_line, which no app may"), "{err}");
}

#[test]
fn a_row_must_carry_its_apps_kernel_name() {
    let mut apps = AppRows::default();
    let err = apps.load("files", NOTES).unwrap_err();
    assert!(err.contains("does not start files_"), "{err}");
    assert!(apps.load("notes", "kai-actions 1\n").is_err());
}

#[test]
fn a_bound_control_finds_only_its_own_apps_row() {
    let mut apps = AppRows::default();
    apps.load("notes", NOTES).unwrap();
    let window = NOTES
        .replace("notes_add", "notes_window_add")
        .replace("notes_count", "notes_window_count")
        .replace("app:notes/", "app:notes-window/");
    apps.load("notes-window", &window).unwrap();
    assert!(apps.find_in("notes-window", "notes_window_add").is_some());
    assert!(apps.find_in("notes", "notes_window_add").is_none());
    assert!(apps.find_in("notes-window", "notes_add").is_none());
    // `notes` may not take `notes-window`'s name for a row of its own.
    let taken = NOTES.replace("notes_count", "notes_window_count");
    let err = apps
        .load("notes", &taken.replacen("version 2", "version 3", 1))
        .unwrap_err();
    assert!(err.contains("notes-window's"), "{err}");
}

#[test]
fn loads_are_bounded_until_reboot() {
    let mut apps = AppRows::default();
    // Each version declares other rows: the same rows load once.
    let version = |label: &str| {
        NOTES
            .replacen("version 2", &format!("version {label}"), 1)
            .replacen("Add a note", &format!("Add a note ({label})"), 1)
    };
    for n in 0..MAX_LOADS {
        apps.load("notes", &version(&n.to_string())).unwrap();
    }
    let err = apps.load("notes", &version("x")).unwrap_err();
    assert!(err.contains("restart"), "{err}");
    assert_eq!(apps.rows().count(), 2);
}
