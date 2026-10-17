use super::*;

fn v1_state(texts: &[&str]) -> Vec<u8> {
    let old = V1 {
        texts: texts.iter().map(|t| t.to_string()).collect(),
    };
    compsdk::state::encode(1, &old)
}

#[test]
fn notes_are_added_counted_found_and_read() {
    let mut notes = Notes::init();
    assert_eq!(notes.handle("add buy milk"), "ok 1");
    assert_eq!(notes.handle("add fix the Roof"), "ok 2");
    assert_eq!(notes.handle("count"), "ok 2 notes, 5 words");
    assert_eq!(notes.handle("find roof"), "ok 2");
    assert_eq!(notes.handle("find nothing"), "ok none");
    assert_eq!(notes.handle("get 1"), "ok buy milk");
    assert_eq!(notes.handle("get 9"), "err no such note");
    assert_eq!(
        notes.handle("about"),
        alloc::format!("ok notes {VERSION} schema 2")
    );
    assert_eq!(notes.handle("find"), "err unknown request \"find\"");
    assert_eq!(notes.health(), 0);
    assert_eq!(notes.self_test(), 0);
}

#[test]
fn version_1_state_migrates_with_ids_and_word_counts() {
    let state = v1_state(&["buy milk", "call bob today", "fix the roof", "walk"]);
    let mut notes = Notes::import(&state, 1).unwrap();
    assert_eq!(notes.handle("count"), "ok 4 notes, 9 words");
    assert_eq!(notes.handle("find roof"), "ok 3");
    assert_eq!(notes.handle("add paint fence"), "ok 5");
    assert_eq!(notes.health(), 0);
}

#[test]
fn its_own_state_round_trips_and_others_are_refused() {
    let mut notes = Notes::init();
    notes.handle("add a b");
    let saved = notes.export(SCHEMA).unwrap();
    assert_eq!(compsdk::state::schema_of(&saved), Some(2));
    assert_eq!(Notes::import(&saved, 2).unwrap(), notes);
    assert_eq!(Notes::import(&saved, 3), Err(ImportError::Unsupported));
    assert_eq!(Notes::import(&saved, 1), Err(ImportError::Bad));
    assert_eq!(notes.export(1), None);
}

#[test]
fn health_finds_wrong_word_counts_and_ids() {
    let mut notes = Notes::init();
    notes.handle("add one two");
    notes.handle("add three");
    let mut wrong = notes.clone();
    wrong.state.notes[0].words = 0;
    assert_eq!(wrong.health(), WORD_COUNTS_WRONG);
    let mut ids = notes.clone();
    ids.state.notes.swap(0, 1);
    assert_eq!(ids.health(), IDS_OUT_OF_ORDER);
    let mut next = notes;
    next.state.next_id = 1;
    assert_eq!(next.health(), IDS_OUT_OF_ORDER);
}
