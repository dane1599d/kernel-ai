//! Ctrl-B: the C component the saved file is in, built by the OS, and its report.

use super::*;

const SOURCE: &str = "/home/user/c/hello/hello.c";

fn ctrl_b(app: &mut EditorWindow, os: &mut Os) {
    press(app, os, "b", "", keys::CTRL);
    serve(app, os);
}

#[test]
fn ctrl_b_in_a_file_that_is_not_in_a_components_folder_says_where_a_component_is() {
    let (mut app, mut os) = window();
    for path in [
        PATH,
        "/home/user/c/hello.c",
        "/home/user/c/Hello/a.c",
        "/home/user/c/h/a.c",
        "/home/user/c/hello/src/a.c",
        "/home/user/c/hello-world-12/a.c",
        "/home/user/d/hello/a.c",
    ] {
        os.disk.insert(
            path.into(),
            b"int x;
"
            .to_vec(),
        );
        open_path(&mut app, &mut os, path);
        os.log.clear();
        ctrl_b(&mut app, &mut os);
        assert!(os.log.is_empty(), "{path}: {:?}", os.log);
        assert!(
            os.status().starts_with("Ctrl-B builds a C component")
                || os.status().contains("is not a component's name"),
            "{path}: {}",
            os.status()
        );
    }
}

#[test]
fn ctrl_b_builds_the_c_component_the_saved_file_is_in_and_shows_the_report() {
    let (mut app, mut os) = window();
    os.disk.insert(
        SOURCE.into(),
        b"int x;
"
        .to_vec(),
    );
    open_path(&mut app, &mut os, SOURCE);
    os.log.clear();
    os.build_answer = Some(Ok(String::from(
        "ok built hello 0.0.1 (5120 bytes) and installed it as generation 2. undo",
    )));
    press(&mut app, &mut os, "b", "", keys::CTRL);
    assert_eq!(os.status(), "building hello...");
    serve(&mut app, &mut os);
    assert_eq!(os.log, ["build hello"]);
    assert_eq!(
        os.status(),
        "built hello 0.0.1 (5120 bytes) and installed it as generation 2. undo"
    );
    // The edit service was not asked, and the file is untouched.
    assert!(os.writes.is_empty());
}

#[test]
fn ctrl_b_wants_the_file_saved_first_because_the_os_builds_what_is_on_disk() {
    let (mut app, mut os) = window();
    os.disk.insert(
        SOURCE.into(),
        b"int x;
"
        .to_vec(),
    );
    open_path(&mut app, &mut os, SOURCE);
    type_text(&mut app, &mut os, "y");
    os.log.clear();
    ctrl_b(&mut app, &mut os);
    assert!(os.log.is_empty(), "{:?}", os.log);
    assert!(
        os.status().contains("Ctrl-S saves first"),
        "{}",
        os.status()
    );
    // A new file that is not on disk yet is no source either.
    save(&mut app, &mut os);
    os.log.clear();
    os.build_answer = Some(Ok(String::from("ok built hello 0.0.1")));
    ctrl_b(&mut app, &mut os);
    assert_eq!(os.log, ["build hello"]);
    open_path(&mut app, &mut os, "/home/user/c/hello/new.c");
    os.log.clear();
    ctrl_b(&mut app, &mut os);
    assert!(os.log.is_empty(), "{:?}", os.log);
    assert!(
        os.status().contains("Ctrl-S saves first"),
        "{}",
        os.status()
    );
}

#[test]
fn a_failed_build_shows_the_compilers_lines_in_the_pager_and_a_refused_one_says_why() {
    let (mut app, mut os) = window();
    os.disk.insert(
        SOURCE.into(),
        b"int x;
"
        .to_vec(),
    );
    open_path(&mut app, &mut os, SOURCE);
    os.build_answer = Some(Ok(String::from(
        "err cc build hello: hello.c:3:5: error: x
hello.c:7:1: error: z",
    )));
    ctrl_b(&mut app, &mut os);
    assert_eq!(os.status(), "cc build hello: hello.c:3:5: error: x");
    let shown = os.rows().join(
        "
",
    );
    assert!(shown.contains("hello.c:7:1: error: z"), "{shown}");
    // Any key closes the pager, and the file is as it was.
    press(&mut app, &mut os, "esc", "", 0);
    assert_eq!(&os.rows()[..1], ["int x;"]);
    // A one-line failure stays in the status row only.
    os.build_answer = Some(Ok(String::from("err cc build hello: pkg: no pkg here")));
    ctrl_b(&mut app, &mut os);
    assert_eq!(os.status(), "cc build hello: pkg: no pkg here");
    assert_eq!(&os.rows()[..1], ["int x;"]);
    // The OS refused before it built anything.
    os.build_answer = Some(Err(String::from(
        "a build needs a key or click of the person at the editor",
    )));
    ctrl_b(&mut app, &mut os);
    assert_eq!(
        os.status(),
        "hello was not built: a build needs a key or click of the person at the editor"
    );
}

#[test]
fn ctrl_s_with_nothing_open_says_what_it_is_for() {
    let (mut app, mut os) = window();
    press(&mut app, &mut os, "s", "", keys::CTRL);
    assert!(os.status().contains("saves a file"), "{}", os.status());
    assert!(os.log.is_empty());
}

#[test]
fn the_agent_s_open_leaves_a_saved_file_for_a_component_and_not_an_edited_one() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    type_text(&mut app, &mut os, "x");
    let refused = app.act_open(&mut os, "notes", "src/lib.rs", 1);
    assert!(refused.unwrap_err().contains("not saved"));
    save(&mut app, &mut os);
    os.log.clear();
    app.act_open(&mut os, "notes", "src/lib.rs", 1).unwrap();
    // The edit service is asked, the file door is not.
    assert!(os.log[0].starts_with("edit "), "{:?}", os.log);
    assert!(
        os.log
            .iter()
            .all(|m| !m.starts_with("read ") && !m.starts_with("write ")),
        "{:?}",
        os.log
    );
}

/// The agent's `editor_open_file`: the same door as Ctrl-O, and an answer that says only
/// that the file is on its way.
fn open_file(app: &mut EditorWindow, os: &mut Os, path: &str) -> Result<String, String> {
    app.act_open_file(os, path)
}

#[test]
fn the_agents_open_file_reads_through_the_door_and_never_returns_the_text() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    let answer = open_file(&mut app, &mut os, PATH).unwrap();
    assert_eq!(answer, format!("Opening {PATH}."));
    assert_eq!(os.log, [format!("read {PATH}")]);
    assert_eq!(os.status(), format!("opening {PATH}..."));
    serve(&mut app, &mut os);
    assert!(os.title().starts_with("file disk: home/user/todo.txt"));
    assert_eq!(&os.rows()[..3], ["buy milk", "call Ann", "sleep"]);
    // What it says afterwards is where it is, never the file's text.
    let said = app.status(&mut os);
    assert!(said.starts_with("file disk: home/user/todo.txt"), "{said}");
    assert!(!said.contains("milk"), "{said}");
    // A write is still the person's: nothing in the action saved anything.
    assert!(os.writes.is_empty());
}

#[test]
fn the_agents_open_file_replaces_a_file_that_is_saved_and_a_path_not_there_is_new() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    press(&mut app, &mut os, "down", "", 0);
    assert!(app.status(&mut os).contains("line 2,"));
    open_file(&mut app, &mut os, PATH).unwrap();
    serve(&mut app, &mut os);
    assert!(app.status(&mut os).contains("line 1,"), "back at the top");
    open_file(&mut app, &mut os, "/home/user/new.txt").unwrap();
    serve(&mut app, &mut os);
    assert!(os.title().contains("new.txt"), "{}", os.title());
}

#[test]
fn the_agents_open_file_is_refused_with_its_reason_and_asks_the_door_nothing() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    os.disk.insert("/home/user/b.txt".into(), b"b\n".to_vec());
    for path in [
        "todo.txt",
        "/home/../etc/passwd",
        "/home/user/a b",
        "/system/log",
        "",
    ] {
        assert!(open_file(&mut app, &mut os, path).is_err(), "{path:?}");
    }
    assert!(os.log.is_empty(), "{:?}", os.log);
    // Edits that are not saved are not opened over.
    open_path(&mut app, &mut os, PATH);
    type_text(&mut app, &mut os, "x");
    os.log.clear();
    let why = open_file(&mut app, &mut os, "/home/user/b.txt").unwrap_err();
    assert!(why.contains("not saved"), "{why}");
    assert!(os.log.is_empty(), "{:?}", os.log);
    assert!(os.title().contains("todo.txt"));
    // One request at a time.
    save(&mut app, &mut os);
    open_file(&mut app, &mut os, "/home/user/b.txt").unwrap();
    let why = open_file(&mut app, &mut os, PATH).unwrap_err();
    assert!(why.contains("still on its way"), "{why}");
    serve(&mut app, &mut os);
    assert!(os.title().contains("b.txt"), "{}", os.title());
}

#[test]
fn the_doors_reason_for_the_agents_file_shows_in_the_status_row() {
    let (mut app, mut os) = window();
    open_file(&mut app, &mut os, "/home/user/c.txt").unwrap();
    let (ticket, _) = os.pending.remove(0);
    app.on_reply(&mut os, ticket, Err("the OS has no file door".into()))
        .unwrap();
    assert_eq!(os.status(), "the OS has no file door");
    assert!(app.status(&mut os).ends_with("the OS has no file door"));
}

/// The window asked to close, as the user's close button does.
fn close_it(app: &mut EditorWindow, os: &mut Os) {
    app.ui_event(os, 7, &UiEvent::CloseRequested).unwrap();
}

fn pressed(app: &mut EditorWindow, os: &mut Os, node: NodeId) {
    app.ui_event(os, 7, &UiEvent::Pressed(node)).unwrap();
}

/// A file open with `x` typed into it: edits the disk does not have.
fn edited() -> (EditorWindow, Os) {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    type_text(&mut app, &mut os, "x");
    (app, os)
}

#[test]
fn the_window_says_it_holds_edits_not_saved_until_they_are() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    // Nothing edited: the host was never told anything.
    assert!(os.unsaved.is_empty(), "{:?}", os.unsaved);
    type_text(&mut app, &mut os, "x");
    assert_eq!(os.unsaved, [true]);
    // More typing is not said again; a save says there is nothing left.
    type_text(&mut app, &mut os, "y");
    assert_eq!(os.unsaved, [true]);
    save(&mut app, &mut os);
    assert_eq!(os.unsaved, [true, false]);
}

#[test]
fn a_window_with_nothing_unsaved_closes_at_once() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    close_it(&mut app, &mut os);
    assert_eq!(os.closed, [7]);
    assert_eq!(app.window(), None);
}

#[test]
fn closing_with_edits_not_saved_asks_and_cancel_goes_back_to_editing() {
    let (mut app, mut os) = edited();
    close_it(&mut app, &mut os);
    assert!(os.closed.is_empty());
    assert!(os.has(SAVE) && os.has(DISCARD) && os.has(CANCEL));
    assert!(os.status().starts_with("Not saved:"), "{}", os.status());
    // Keys are answers while it asks, and are not typed.
    let before = os.rows();
    type_text(&mut app, &mut os, "zz");
    assert_eq!(os.rows(), before);
    pressed(&mut app, &mut os, CANCEL);
    assert!(!os.has(SAVE) && !os.has(DISCARD) && !os.has(CANCEL));
    assert!(os.closed.is_empty());
    // Typing goes to the file again, and the flag was never withdrawn.
    type_text(&mut app, &mut os, "q");
    assert!(os.rows()[0].starts_with("xqbuy milk"), "{:?}", os.rows());
    assert_eq!(os.unsaved, [true]);
}

#[test]
fn discard_closes_the_window_and_leaves_the_file_as_it_was_saved() {
    let (mut app, mut os) = edited();
    close_it(&mut app, &mut os);
    pressed(&mut app, &mut os, DISCARD);
    assert_eq!(os.closed, [7]);
    assert_eq!(os.disk[PATH], todo());
    // A window opened again is an editor with no file open.
    app.present(&mut os).unwrap();
    assert!(os.status().contains("Ctrl-O opens one"), "{}", os.status());
    assert!(!os.has(SAVE));
    assert_eq!(os.log, [format!("read {PATH}")]);
}

#[test]
fn save_writes_the_file_and_closes_the_window_when_it_is_written() {
    let (mut app, mut os) = edited();
    close_it(&mut app, &mut os);
    pressed(&mut app, &mut os, SAVE);
    // The write is out; the window waits for it, and keys are not typed.
    assert!(os.closed.is_empty());
    assert!(
        os.pending
            .iter()
            .any(|(_, r)| matches!(r, Req::Write { .. }))
    );
    type_text(&mut app, &mut os, "zz");
    serve(&mut app, &mut os);
    assert!(
        os.disk[PATH].starts_with(b"xbuy milk\r\n"),
        "{:?}",
        os.disk[PATH]
    );
    assert_eq!(os.closed, [7]);
    assert_eq!(app.window(), None);
}

#[test]
fn a_save_the_os_refuses_keeps_the_window_and_says_why() {
    let (mut app, mut os) = edited();
    os.refuse = Some("fs_write asks first: ask the AI");
    close_it(&mut app, &mut os);
    pressed(&mut app, &mut os, SAVE);
    serve(&mut app, &mut os);
    assert!(os.closed.is_empty());
    assert_eq!(os.status(), "not saved: fs_write asks first: ask the AI");
    assert!(
        !os.has(SAVE),
        "the question is over; the edits are still there"
    );
    assert_eq!(os.disk[PATH], todo());
    // Closing again asks again.
    close_it(&mut app, &mut os);
    assert!(os.has(SAVE));
}

#[test]
fn the_keys_s_d_and_escape_answer_the_question() {
    let (mut app, mut os) = edited();
    close_it(&mut app, &mut os);
    press(&mut app, &mut os, "esc", "", 0);
    assert!(os.closed.is_empty() && !os.has(CANCEL));
    close_it(&mut app, &mut os);
    // Ctrl-S is not S.
    press(&mut app, &mut os, "s", "", keys::CTRL);
    assert!(os.pending.is_empty() && os.closed.is_empty());
    press(&mut app, &mut os, "s", "s", 0);
    serve(&mut app, &mut os);
    assert_eq!(os.closed, [7]);

    let (mut app, mut os) = edited();
    close_it(&mut app, &mut os);
    press(&mut app, &mut os, "d", "d", 0);
    assert_eq!(os.closed, [7]);
    assert_eq!(os.disk[PATH], todo());
}

/// A new editor from `app`'s checkpoint, as a nap's wake makes one, on the same
/// disk: the import, which asks the door for nothing, and the first call after it,
/// which reads the file; the answer is served.
fn woken(app: &EditorWindow, disk: &BTreeMap<String, Vec<u8>>) -> (EditorWindow, Os) {
    let kept = app.export(comp_editor_window::SCHEMA).unwrap();
    let mut os = Os {
        disk: disk.clone(),
        ..Os::default()
    };
    let mut woke = EditorWindow::import(&kept, comp_editor_window::SCHEMA).unwrap();
    assert!(os.pending.is_empty() && os.log.is_empty());
    woke.on_size(&mut os, 10, 60).unwrap();
    serve(&mut woke, &mut os);
    (woke, os)
}

#[test]
fn text_not_saved_is_in_the_checkpoint_and_a_woken_editor_has_it_back_unsaved() {
    // "x" is typed at the start of line 1 and is still the line being typed.
    let (app, os) = edited();
    let kept = app.export(comp_editor_window::SCHEMA).unwrap();
    let state: comp_editor_window::V1 =
        compsdk::state::decode(&kept, comp_editor_window::SCHEMA).unwrap();
    assert_eq!(state.path.as_deref(), Some(PATH));
    assert_eq!(state.text.as_deref(), Some("xbuy milk\r\ncall Ann\nsleep"));
    assert_eq!(state.line, Some(1));
    assert!(kept.len() < compsdk::MAX_STATE);

    let (mut woke, mut back) = woken(&app, &os.disk);
    assert_eq!(back.log, [format!("read {PATH}")]);
    assert_eq!(&back.rows()[..2], ["xbuy milk", "call Ann"]);
    // It says it holds text not saved, as the first one did, and the disk was
    // not written.
    assert_eq!(back.unsaved, [true]);
    assert!(back.writes.is_empty());
    assert_eq!(back.disk[PATH], todo());
    // Ctrl-S writes what was kept.
    save(&mut woke, &mut back);
    assert_eq!(back.disk[PATH], b"xbuy milk\r\ncall Ann\nsleep");
    assert_eq!(back.unsaved, [true, false]);
}

#[test]
fn the_cursors_line_and_edits_already_made_come_back_too() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    // An edit that went to the file (the cursor moved off its line), and a
    // line being typed on the next.
    type_text(&mut app, &mut os, "A");
    press(&mut app, &mut os, "down", "", 0);
    type_text(&mut app, &mut os, "B");
    let kept = app.export(comp_editor_window::SCHEMA).unwrap();
    let state: comp_editor_window::V1 =
        compsdk::state::decode(&kept, comp_editor_window::SCHEMA).unwrap();
    assert_eq!(state.line, Some(2));
    assert_eq!(state.text.as_deref(), Some("Abuy milk\r\ncBall Ann\nsleep"));
    let (mut woke, mut back) = woken(&app, &os.disk);
    assert_eq!(&back.rows()[..2], ["Abuy milk", "cBall Ann"]);
    assert!(woke.status(&mut back).contains("line 2"));
    // One undo takes the file back toward the disk's, not to nothing.
    press(&mut woke, &mut back, "z", "", keys::CTRL);
    serve(&mut woke, &mut back);
    assert_eq!(back.rows()[0], "buy milk");
}

#[test]
fn an_editor_woken_before_its_first_call_keeps_what_it_was_given() {
    let (app, os) = edited();
    let kept = app.export(comp_editor_window::SCHEMA).unwrap();
    // Imported and exported again, as an editor that napped twice in a row.
    let woke = EditorWindow::import(&kept, comp_editor_window::SCHEMA).unwrap();
    let again = woke.export(comp_editor_window::SCHEMA).unwrap();
    assert_eq!(again, kept);
    let (_, back) = woken(&woke, &os.disk);
    assert_eq!(&back.rows()[..1], ["xbuy milk"]);
}

#[test]
fn the_kept_text_wins_over_a_file_that_changed_or_went_while_the_editor_napped() {
    let (app, os) = edited();
    // The disk's file changed.
    let mut disk = os.disk.clone();
    disk.insert(PATH.into(), b"something else\n".to_vec());
    let (_, back) = woken(&app, &disk);
    assert_eq!(&back.rows()[..2], ["xbuy milk", "call Ann"]);
    assert_eq!(back.unsaved, [true]);
    // The file is gone: the text is shown as a new file, and a save makes it.
    disk.clear();
    let (mut woke, mut back) = woken(&app, &disk);
    assert_eq!(&back.rows()[..2], ["xbuy milk", "call Ann"]);
    assert_eq!(back.unsaved, [true]);
    save(&mut woke, &mut back);
    assert_eq!(back.writes, [(true, "xbuy milk\r\ncall Ann\nsleep".len())]);
}

#[test]
fn a_file_that_cannot_be_read_again_still_shows_the_text_that_was_kept() {
    let (app, _) = edited();
    let mut os = Os::default();
    let kept = app.export(comp_editor_window::SCHEMA).unwrap();
    let mut woke = EditorWindow::import(&kept, comp_editor_window::SCHEMA).unwrap();
    woke.on_size(&mut os, 10, 60).unwrap();
    let (ticket, _) = os.pending.remove(0);
    woke.on_reply(&mut os, ticket, Err(String::from("denied: not yours")))
        .unwrap();
    serve(&mut woke, &mut os);
    assert_eq!(&os.rows()[..1], ["xbuy milk"]);
    // The text is open under a name of its own beside the file, which says so.
    let there = format!("{PATH}.recovered-0");
    assert!(os.status().contains(&there), "{}", os.status());
    assert!(woke.status(&mut os).contains("recovered-0"));
    // The file is there after all; a save makes the new name and leaves it be.
    os.disk.insert(PATH.into(), todo());
    save(&mut woke, &mut os);
    assert_eq!(os.disk[PATH], todo());
    let kept: &[u8] = b"xbuy milk\r\ncall Ann\nsleep";
    assert_eq!(os.disk[&there], kept);
    assert_eq!(os.writes, [(true, kept.len())]);
    assert!(os.status().contains("saved"), "{}", os.status());
}

#[test]
fn a_file_the_agent_names_takes_the_place_of_the_one_a_nap_kept() {
    let (_, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    os.disk
        .insert("/home/user/other.txt".into(), b"other".to_vec());
    let kept = comp_editor_window::V1 {
        path: Some(PATH.into()),
        ..comp_editor_window::V1::default()
    };
    let bytes = compsdk::state::encode(comp_editor_window::SCHEMA, &kept);
    let mut woke = EditorWindow::import(&bytes, comp_editor_window::SCHEMA).unwrap();
    woke.act_open_file(&mut os, "/home/user/other.txt").unwrap();
    serve(&mut woke, &mut os);
    // Only the file named is read, not the kept one after it.
    woke.on_size(&mut os, 24, 80).unwrap();
    serve(&mut woke, &mut os);
    assert_eq!(os.log, ["read /home/user/other.txt"]);
}

#[test]
fn the_checkpoint_keeps_the_open_file_and_a_reopened_editor_reads_it_again() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    let kept = app.export(comp_editor_window::SCHEMA).unwrap();
    let state: comp_editor_window::V1 =
        compsdk::state::decode(&kept, comp_editor_window::SCHEMA).unwrap();
    assert_eq!(state.path.as_deref(), Some(PATH));
    // Nothing was typed: there is no text to keep.
    assert_eq!(state.text, None);
    // A new editor, as a nap's wake makes one. The import asks the door for
    // nothing (the host takes a file request only from a call a transaction
    // wraps, and a migration is not one); the first call that is one reads the
    // file again, and the second reads nothing more.
    let (_, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    let mut woke = EditorWindow::import(&kept, comp_editor_window::SCHEMA).unwrap();
    assert!(os.pending.is_empty() && os.log.is_empty());
    woke.on_size(&mut os, 24, 80).unwrap();
    serve(&mut woke, &mut os);
    assert_eq!(os.log, [format!("read {PATH}")]);
    assert_eq!(&os.rows()[..2], ["buy milk", "call Ann"]);
    woke.on_size(&mut os, 20, 80).unwrap();
    serve(&mut woke, &mut os);
    assert_eq!(os.log.len(), 1, "{:?}", os.log);
    // Nothing open, nothing kept.
    let (empty, _) = window();
    let bytes = empty.export(comp_editor_window::SCHEMA).unwrap();
    let nothing: comp_editor_window::V1 =
        compsdk::state::decode(&bytes, comp_editor_window::SCHEMA).unwrap();
    assert_eq!(nothing.path, None);
}
