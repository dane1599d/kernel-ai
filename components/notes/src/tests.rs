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

#[test]
fn a_list_is_each_notes_id_and_first_line() {
    let mut notes = Notes::init();
    assert!(notes.list().is_empty());
    notes.add("buy milk\nand eggs").unwrap();
    notes.add("").unwrap();
    notes
        .add("\n  \nsecond line is the first that counts")
        .unwrap();
    notes.add(&"long ".repeat(40)).unwrap();
    let list = notes.list();
    assert_eq!(list[0], "1: buy milk");
    assert_eq!(list[1], "2: (empty)");
    assert_eq!(list[2], "3: second line is the first that counts");
    assert!(
        list[3].starts_with("4: long long") && list[3].ends_with('…'),
        "{list:?}"
    );
    assert!(first_line(&"x".repeat(LIST_CHARS), LIST_CHARS).ends_with('x'));
}

#[test]
fn many_notes_list_the_first_hundred_and_the_reply_fits_one_message() {
    let mut notes = Notes::init();
    for _ in 0..300 {
        notes.add(&"w".repeat(100)).unwrap();
    }
    let list = notes.list();
    assert_eq!(list.len(), MAX_LISTED);
    let bytes: usize = list.iter().map(|line| line.len() + 2).sum();
    assert!(bytes < kabi::MAX_MESSAGE - 64, "{bytes} bytes");
}

#[test]
fn remove_takes_a_note_by_its_id_and_the_others_keep_theirs() {
    let mut notes = Notes::init();
    notes.add("one").unwrap();
    notes.add("two words").unwrap();
    notes.add("three").unwrap();
    assert_eq!(notes.remove(2), Ok(String::from("Removed note 2.")));
    assert_eq!(notes.list(), ["1: one", "3: three"]);
    assert_eq!(notes.count(), tally(2, 2));
    assert_eq!(notes.get(2), None);
    assert_eq!(notes.get(3).as_deref(), Some("three"));
    // The id is not given again, and the state is as healthy as it was.
    assert_eq!(notes.add("four"), Ok(4));
    assert_eq!(notes.health(), 0);
    assert_eq!(
        notes.remove(2),
        Err(String::from("there is no note 2")),
        "a note is removed once"
    );
}

#[test]
fn append_adds_a_line_and_edit_replaces_the_text() {
    let mut notes = Notes::init();
    notes.add("shopping").unwrap();
    notes.add("").unwrap();
    assert_eq!(
        notes.append(1, "milk"),
        Ok(String::from("Added to note 1."))
    );
    assert!(notes.append(1, "and eggs").is_ok());
    assert_eq!(notes.get(1).as_deref(), Some("shopping\nmilk\nand eggs"));
    // An empty note takes the text with no blank line before it.
    assert!(notes.append(2, "first words").is_ok());
    assert_eq!(notes.get(2).as_deref(), Some("first words"));
    assert_eq!(notes.count(), tally(2, 6));
    assert_eq!(
        notes.edit(1, "buy bread"),
        Ok(String::from("Changed note 1."))
    );
    assert_eq!(notes.get(1).as_deref(), Some("buy bread"));
    assert_eq!(notes.count(), tally(2, 4));
    assert_eq!(notes.health(), 0);
    assert_eq!(notes.find("bread"), [1]);
    assert!(notes.find("milk").is_empty());
}

#[test]
fn append_and_edit_refuse_what_is_not_there_is_empty_or_does_not_fit() {
    let mut notes = Notes::init();
    notes.add("a").unwrap();
    let nothing = String::from("there is no note 9");
    assert_eq!(notes.append(9, "x"), Err(nothing.clone()));
    assert_eq!(notes.edit(9, "x"), Err(nothing));
    assert_eq!(
        notes.append(1, "   "),
        Err(String::from("there is nothing to add"))
    );
    // A note holds at most MAX_NOTE bytes, however it grows.
    let big = "b".repeat(MAX_NOTE);
    assert!(notes.edit(1, &big).is_ok());
    let long = String::from("a note holds at most 2000 bytes");
    assert_eq!(notes.append(1, "c"), Err(long.clone()));
    assert_eq!(notes.edit(1, &format!("{big}c")), Err(long));
    assert_eq!(notes.get(1).as_deref(), Some(big.as_str()));
    // Editing to nothing is a note with no words.
    assert!(notes.edit(1, "").is_ok());
    assert_eq!(notes.count(), tally(1, 0));
}

#[test]
fn a_note_that_would_not_fit_the_state_is_refused_and_left_as_it_was() {
    let mut notes = Notes::init();
    notes.add("a").unwrap();
    let text = "x ".repeat(MAX_NOTE / 2);
    while notes.add(&text).is_ok() {}
    while notes.add("q").is_ok() {}
    let before = notes.clone();
    assert_eq!(
        notes.edit(1, &text),
        Err(String::from("notes are full")),
        "{}",
        notes.export(SCHEMA).unwrap().len()
    );
    assert_eq!(notes, before);
    // A shorter text always fits.
    assert!(notes.edit(2, "short").is_ok());
}

/// A version from before the actions answers with nothing and refuses changes.
#[test]
fn an_older_version_answers_the_new_actions_without_trapping() {
    struct Old;
    impl Actions for Old {
        fn add(&mut self, _: &str) -> Result<u32, String> {
            Ok(1)
        }
        fn count(&self) -> Tally {
            tally(0, 0)
        }
        fn find(&self, _: &str) -> Vec<u32> {
            Vec::new()
        }
        fn get(&self, _: u32) -> Option<String> {
            None
        }
        fn about(&self) -> String {
            String::new()
        }
    }
    let mut old = Old;
    assert!(old.list().is_empty());
    assert!(old.remove(1).unwrap_err().contains("cannot remove"));
    assert!(old.append(1, "x").unwrap_err().contains("cannot add"));
    assert!(old.edit(1, "x").unwrap_err().contains("cannot edit"));
}
