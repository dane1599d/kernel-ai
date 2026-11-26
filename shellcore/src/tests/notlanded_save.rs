//! A change made outside a typed line whose file action `fs` did not commit
//! (#1306's M5): the editor's save is an error, never a save; an agent's
//! asked step is told it did not land.

use super::agentstep::step;
use super::*;
use crate::door::DOOR_APP;

#[test]
fn an_editor_save_that_did_not_land_is_an_error() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("todo.txt".into(), "old\n".into());
    let mut shell = shell(&host);
    host.one_action.set(true);
    host.commit_refused.set(true);
    let saved = shell.door_write(DOOR_APP, "/home/user/todo.txt", b"new\n", false, true);
    let why = saved.unwrap_err();
    assert!(
        why.starts_with("/home/user/todo.txt was not saved: "),
        "{why}"
    );
    assert!(why.contains("did not land"), "{why}");
    assert_eq!(host.files.borrow()["todo.txt"], "old\n");
    // Its entry names no file change: an undo of it restores nothing.
    let id = shell.log.last_id();
    let entry = shell.log.get(id).unwrap();
    assert!(entry.changes.is_empty(), "{entry:?}");
    assert!(entry.outcome.contains("did not land"), "{}", entry.outcome);
    // The next save lands, and says nothing of the last.
    host.commit_refused.set(false);
    shell
        .door_write(DOOR_APP, "/home/user/todo.txt", b"newer\n", false, true)
        .unwrap();
    assert_eq!(host.files.borrow()["todo.txt"], "newer\n");
}

#[test]
fn an_agents_asked_file_step_that_did_not_land_is_told_so() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let manifest = kaiagent::Manifest::parse(
        "kai-app 1\nid files-agent\nabi 1.0\nagent 1\nclass background\n\
         model tokens/day=50000 kv=1024 pool=64m\nactions fs_create\n\
         memory none\nnet none\n",
    )
    .unwrap();
    let badge = u64::from(shell.roster_mut().install(manifest).unwrap().badge());
    host.one_action.set(true);
    let create = r#"{"name":"fs_create","arguments":{"path":"a.txt","text":"x"}}"#;
    let asked = step(&mut shell, badge, create);
    assert!(asked.starts_with("ok asked "), "{asked}");
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).expect("the agent's card");
    let once = host.mint(menu.id, menu.call());
    host.commit_refused.set(true);
    let ran = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(!ran.ok && ran.text.contains("did not land"), "{ran:?}");
    let told = step(&mut shell, badge, &format!("wait {}", menu.id));
    assert!(
        told.starts_with("err ") && told.contains("did not land"),
        "{told}"
    );
    assert!(host.files.borrow().is_empty());
}
