use super::*;

fn v1_state(texts: &[&str]) -> Vec<u8> {
    let old = V1 {
        texts: texts.iter().map(|t| t.to_string()).collect(),
    };
    compsdk::state::encode(1, &old)
}

fn tally(notes: u32, words: u32) -> Tally {
    Tally { notes, words }
}

#[test]
fn notes_are_added_counted_found_and_read() {
    let mut notes = Notes::init();
    assert_eq!(notes.add("buy milk"), Ok(1));
    assert_eq!(notes.add("fix the Roof"), Ok(2));
    assert_eq!(notes.count(), tally(2, 5));
    assert_eq!(notes.find("roof"), [2]);
    assert!(notes.find("nothing").is_empty());
    assert_eq!(notes.get(1).as_deref(), Some("buy milk"));
    assert_eq!(notes.get(9), None);
    assert_eq!(notes.about(), alloc::format!("notes {VERSION} schema 2"));
    assert_eq!(notes.health(), 0);
    assert_eq!(notes.self_test(), 0);
}

#[test]
fn version_1_state_migrates_with_ids_and_word_counts() {
    let state = v1_state(&["buy milk", "call bob today", "fix the roof", "walk"]);
    let mut notes = Notes::import(&state, 1).unwrap();
    assert_eq!(notes.count(), tally(4, 9));
    assert_eq!(notes.find("roof"), [3]);
    assert_eq!(notes.add("paint fence"), Ok(5));
    assert_eq!(notes.health(), 0);
}

#[test]
fn its_own_state_round_trips_and_others_are_refused() {
    let mut notes = Notes::init();
    notes.add("a b").unwrap();
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
    notes.add("one two").unwrap();
    notes.add("three").unwrap();
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

#[test]
fn the_state_and_the_replies_stay_within_what_the_host_keeps() {
    let mut notes = Notes::init();
    let text = "x ".repeat(2000);
    let mut added = 0;
    while notes.add(&text).is_ok() {
        added += 1;
        assert!(added < 1000, "notes never filled up");
    }
    assert_eq!(notes.add(&text), Err(String::from("notes are full")));
    assert!(notes.export(SCHEMA).unwrap().len() <= compsdk::MAX_STATE);
    // Many matches: at most MAX_FOUND ids, whose WAVE list fits one message.
    let mut many = Notes::init();
    for _ in 0..3000 {
        many.add("x").unwrap();
    }
    let found = many.find("x");
    assert_eq!(found.len(), MAX_FOUND);
    let wave_len: usize = found.iter().map(|id| id.to_string().len() + 2).sum();
    assert!(wave_len < kabi::MAX_MESSAGE - 64, "{wave_len} bytes");
}
