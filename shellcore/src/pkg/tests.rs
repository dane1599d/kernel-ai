use super::*;
use alloc::string::ToString;
use alloc::vec;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| n.to_string()).collect()
}

#[test]
fn an_install_brings_its_dependencies_and_undo_switches_back() {
    let mut store = Store::default();
    let switch = store.install(&names(&["notes"])).unwrap();
    assert_eq!(
        switch.what(),
        "installed notes 2.0.0 (74 KB), with spell-en 1.2.3"
    );
    assert!(switch.grants.is_empty());
    assert_eq!(store.commit(switch), Ok(1));
    assert_eq!(store.current(), 2);
    assert_eq!(
        store.list(),
        "notes 2.0.0\nspell-en 1.2.3 (needed by another)"
    );
    assert_eq!(
        store.install(&names(&["notes"])),
        Err(String::from("notes 2.0.0 is already installed"))
    );
    // Undo: back to the parent, only while the install is still current.
    assert_eq!(store.switch_back(2, 1), Ok(()));
    assert_eq!(store.list(), "nothing installed beyond the system");
    assert_eq!(store.switch_back(2, 1), Err(1));
    // Redo: forward again.
    assert_eq!(store.switch_back(1, 2), Ok(()));
    assert_eq!(store.current(), 2);
}

#[test]
fn a_remove_takes_the_dependencies_nothing_else_needs() {
    let mut store = Store::default();
    store
        .commit(store.install(&names(&["notes", "spell-en"])).unwrap())
        .unwrap();
    let switch = store.remove(&names(&["notes"])).unwrap();
    assert_eq!(switch.what(), "removed notes 2.0.0");
    store.commit(switch).unwrap();
    assert_eq!(store.list(), "spell-en 1.2.3");
    store
        .commit(store.remove(&names(&["spell-en"])).unwrap())
        .unwrap();
    assert_eq!(
        store.remove(&names(&["spell-en"])),
        Err(String::from("spell-en is not installed"))
    );
    let mut store = Store::default();
    store
        .commit(store.install(&names(&["notes"])).unwrap())
        .unwrap();
    assert_eq!(
        store.remove(&names(&["notes"])).unwrap().what(),
        "removed notes 2.0.0, and spell-en 1.2.3"
    );
}

#[test]
fn what_needs_a_permission_says_so_and_unknown_names_say_why() {
    let store = Store::default();
    assert_eq!(
        store.install(&names(&["weather"])).unwrap().grants,
        ["network"]
    );
    assert_eq!(
        store.install(&names(&["python3"])),
        Err(String::from(
            "No Kernel AI package named python3. Linux packages need the Linux layer (v0.2)."
        ))
    );
    assert_eq!(
        store.info("weather"),
        "weather 0.1.0 (app, 38 KB, MIT): Today's weather where you are. Needs network."
    );
    assert_eq!(
        store.search("word"),
        "spell-en 1.2.3: An English word list for spelling"
    );
}

#[test]
fn a_switch_worked_out_before_another_is_not_made() {
    let mut store = Store::default();
    let first = store.install(&names(&["notes"])).unwrap();
    let second = store.install(&names(&["weather"])).unwrap();
    store.commit(first).unwrap();
    assert!(store.commit(second).is_err());
    assert_eq!(
        store.list(),
        "notes 2.0.0\nspell-en 1.2.3 (needed by another)"
    );
}

#[test]
fn history_is_append_only_and_a_rollback_is_a_new_generation() {
    let mut store = Store::default();
    store
        .commit(store.install(&names(&["notes"])).unwrap())
        .unwrap();
    store
        .commit(store.install(&names(&["weather"])).unwrap())
        .unwrap();
    let back = store.rollback(Some(1)).unwrap();
    assert_eq!(
        back.what(),
        "removed notes 2.0.0, and spell-en 1.2.3, and weather 0.1.0"
    );
    store.commit(back).unwrap();
    assert_eq!(
        store.history(),
        "1: image, 0 packages\n2: install notes, 2 packages\n3: install weather, 3 packages\n4: rollback 1, 0 packages (current)"
    );
    assert!(store.rollback(Some(9)).is_err());
    let mut store = Store::default();
    assert_eq!(
        store.rollback(None),
        Err(String::from("generation 1 has nothing before it"))
    );
    store
        .commit(store.install(&names(&["notes"])).unwrap())
        .unwrap();
    assert_eq!(store.rollback(None).unwrap().to.installed, vec![]);
}
