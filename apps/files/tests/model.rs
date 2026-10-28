//! The Files model on the host: what it asks the OS for after each choice, what
//! it makes of the answers, and that hostile listings, text and messages change
//! nothing but what the window shows. The fake queue gives tickets from 1.

#[path = "../../../testkit/hostile.rs"]
mod hostile;

use comp_files::fake::Queue;
use comp_files::names::{Kind, MAX_ITEMS, name_ok};
use comp_files::{Files, MESSAGE_CHARS, PREVIEW_CHARS, PREVIEW_LINES, Preview, Status};

struct App {
    files: Files,
    os: Queue,
}

impl App {
    fn new() -> Self {
        Self {
            files: Files::new(),
            os: Queue::default(),
        }
    }

    /// The home folder, listed and answered.
    fn with(listing: &str) -> Self {
        let mut app = Self::new();
        app.files.start(&mut app.os);
        app.answer(Ok(listing));
        app
    }

    /// The newest call, as `name args`.
    fn last(&self) -> String {
        self.os.last().expect("a call").1
    }

    /// Answers the newest call.
    fn answer(&mut self, outcome: Result<&str, &str>) -> bool {
        let (ticket, _) = self.os.last().expect("a call");
        self.answer_to(ticket, outcome)
    }

    fn answer_to(&mut self, ticket: u32, outcome: Result<&str, &str>) -> bool {
        let outcome = outcome.map(String::from).map_err(String::from);
        self.files.done(&mut self.os, ticket, outcome)
    }

    fn names(&self) -> Vec<String> {
        self.files
            .items()
            .iter()
            .map(|i| match i.kind {
                Kind::Dir => format!("{}/", i.name),
                Kind::File => i.name.clone(),
            })
            .collect()
    }
}

#[test]
fn the_home_folder_lists_and_a_file_previews() {
    let mut app = App::new();
    app.files.start(&mut app.os);
    assert_eq!(app.last(), r#"fs_list {path: ""}"#);
    assert!(app.files.busy());
    assert!(app.answer(Ok("notes.txt\ndocs/\nZed.md\n")));
    assert!(!app.files.busy());
    assert_eq!(app.names(), ["docs/", "notes.txt", "Zed.md"]);

    app.files.select(&mut app.os, "notes.txt").unwrap();
    assert_eq!(app.last(), r#"fs_read {path: "notes.txt"}"#);
    assert_eq!(app.files.preview(), &Preview::Loading);
    assert_eq!(app.files.rename_text(), "notes.txt");
    assert!(app.answer(Ok("milk\neggs\n")));
    assert_eq!(
        app.files.preview(),
        &Preview::Text {
            text: "milk\neggs\n".into(),
            cut: false
        }
    );
    assert_eq!(app.files.selected_index(), Some(1));
}

#[test]
fn a_folder_previews_its_names_and_opens_on_open() {
    let mut app = App::with("docs/\nnotes.txt\n");
    app.files.select(&mut app.os, "docs").unwrap();
    assert_eq!(app.last(), r#"fs_list {path: "docs"}"#);
    app.answer(Ok("old/\na.txt\nb.txt"));
    assert_eq!(
        app.files.preview(),
        &Preview::Folder {
            names: vec!["old/".into(), "a.txt".into(), "b.txt".into()],
            count: 3
        }
    );

    app.files.open_selected(&mut app.os);
    assert_eq!(app.last(), r#"fs_list {path: "docs"}"#);
    // Until the answer comes, the old folder stays shown.
    assert_eq!(app.files.dir(), "");
    app.answer(Ok("old/\na.txt\nb.txt"));
    assert_eq!(app.files.dir(), "docs");
    assert_eq!(app.files.selected(), None);

    // Up lists the folder above and selects the one it came from.
    app.files.up(&mut app.os);
    assert_eq!(app.last(), r#"fs_list {path: ""}"#);
    app.answer(Ok("docs/\nnotes.txt\n"));
    assert_eq!(app.files.dir(), "");
    assert_eq!(app.files.selected(), Some("docs"));
    assert_eq!(app.last(), r#"fs_list {path: "docs"}"#);

    // At home, Up asks for nothing.
    let before = app.os.calls.len();
    app.files.up(&mut app.os);
    assert_eq!(app.os.calls.len(), before);
}

#[test]
fn opening_a_file_does_nothing_and_a_missing_folder_says_why() {
    let mut app = App::with("notes.txt\ndocs/\n");
    app.files.select(&mut app.os, "notes.txt").unwrap();
    let before = app.os.calls.len();
    app.files.open_selected(&mut app.os);
    assert_eq!(app.os.calls.len(), before);

    app.files.select(&mut app.os, "docs").unwrap();
    app.files.open_selected(&mut app.os);
    assert!(app.answer(Err("docs: not found")));
    assert_eq!(app.files.dir(), "");
    assert_eq!(
        app.files.status(),
        &Status::Failed("docs: not found".into())
    );
}

#[test]
fn an_answer_to_an_old_question_is_dropped() {
    let mut app = App::with("a.txt\nb.txt\n");
    app.files.select(&mut app.os, "a.txt").unwrap();
    let first = app.os.calls.len() as u32;
    app.files.select(&mut app.os, "b.txt").unwrap();
    assert!(!app.answer_to(first, Ok("text of a")));
    assert_eq!(app.files.preview(), &Preview::Loading);
    assert!(app.answer(Ok("text of b")));
    assert!(matches!(app.files.preview(), Preview::Text { text, .. } if text == "text of b"));

    // A listing that a newer one replaced does not change the folder.
    app.files.refresh(&mut app.os);
    let stale = app.os.calls.len() as u32;
    app.files.refresh(&mut app.os);
    let fresh = app.os.calls.len() as u32;
    assert!(!app.answer_to(stale, Ok("old/")));
    assert!(app.answer_to(
        fresh,
        Ok("a.txt
b.txt
c.txt")
    ));
    assert_eq!(app.files.items().len(), 3);
    // An answer to a ticket it never asked is nothing.
    assert!(!app.answer_to(9_999, Ok("x")));
}

#[test]
fn a_name_in_the_field_renames_and_a_path_moves() {
    let mut app = App::with("docs/\nnotes.txt\n");
    app.files.select(&mut app.os, "notes.txt").unwrap();
    app.answer(Ok("hi"));

    app.files.set_rename("plan.txt");
    app.files.move_selected(&mut app.os);
    assert_eq!(app.last(), r#"fs_move {from: "notes.txt", to: "plan.txt"}"#);
    assert!(app.answer(Ok("")));
    assert_eq!(
        app.files.status(),
        &Status::Done("Moved to plan.txt.".into())
    );
    // The folder is listed again, with the moved file selected.
    assert_eq!(app.last(), r#"fs_list {path: ""}"#);
    app.answer(Ok("docs/\nplan.txt"));
    assert_eq!(app.files.selected(), Some("plan.txt"));

    // `folder/` moves it in under its own name.
    app.answer(Ok("hi"));
    app.files.set_rename("docs/");
    app.files.move_selected(&mut app.os);
    assert_eq!(
        app.last(),
        r#"fs_move {from: "plan.txt", to: "docs/plan.txt"}"#
    );
    app.answer(Ok(""));
    app.answer(Ok("docs/"));
    assert_eq!(app.files.selected(), None);

    // A path from home names the new place.
    let mut app = App::with("a.txt\n");
    app.files.select(&mut app.os, "a.txt").unwrap();
    app.files.set_rename("docs/b.txt");
    app.files.move_selected(&mut app.os);
    assert_eq!(app.last(), r#"fs_move {from: "a.txt", to: "docs/b.txt"}"#);
}

#[test]
fn a_move_it_cannot_make_asks_nothing() {
    let mut app = App::with("a.txt\nb.txt\n");
    let n = app.os.calls.len();
    app.files.move_selected(&mut app.os);
    assert_eq!(
        app.files.status(),
        &Status::Failed("Pick something to move first.".into())
    );
    app.files.select(&mut app.os, "a.txt").unwrap();
    let n = n + 1;
    for (text, why) in [
        ("", "Type where to move it, or a new name."),
        ("   ", "Type where to move it, or a new name."),
        ("a.txt", "It is already called that."),
        ("b.txt", "There is already one with that name."),
        ("../out", "That is not a name or path I can use."),
        ("x/../y", "That is not a name or path I can use."),
        ("/abs", "That is not a name or path I can use."),
        ("bad\nname", "That is not a name or path I can use."),
        ("docs//", "That is not a name or path I can use."),
    ] {
        app.files.set_rename(text);
        app.files.move_selected(&mut app.os);
        assert_eq!(app.files.status(), &Status::Failed(why.into()), "{text:?}");
        assert_eq!(app.os.calls.len(), n, "{text:?}");
    }
}

#[test]
fn trash_and_restore_ask_the_os_and_remember_the_path() {
    let mut app = App::with("docs/\nnotes.txt\n");
    app.files.restore_last(&mut app.os);
    assert!(matches!(app.files.status(), Status::Failed(_)));
    assert!(!app.files.can_restore());

    app.files.select(&mut app.os, "notes.txt").unwrap();
    app.answer(Ok("hi"));
    app.files.trash_selected(&mut app.os);
    assert_eq!(app.last(), r#"fs_trash {path: "notes.txt"}"#);
    assert!(!app.files.can_restore());
    app.answer(Ok(""));
    assert!(app.files.can_restore());
    assert!(
        matches!(app.files.status(), Status::Done(s) if s.starts_with("notes.txt is in the trash"))
    );
    app.answer(Ok("docs/"));
    assert_eq!(app.files.selected(), None);

    app.files.restore_last(&mut app.os);
    assert_eq!(app.last(), r#"fs_restore {path: "notes.txt"}"#);
    app.answer(Ok(""));
    assert!(!app.files.can_restore());
    app.answer(Ok("docs/\nnotes.txt"));
    assert_eq!(app.files.selected(), Some("notes.txt"));
}

#[test]
fn a_refused_restore_keeps_the_path_for_another_try() {
    let mut app = App::with("a.txt\n");
    app.files.select(&mut app.os, "a.txt").unwrap();
    app.files.trash_selected(&mut app.os);
    app.answer(Ok(""));
    app.files.restore_last(&mut app.os);
    app.answer(Err("a.txt exists again; move it away first"));
    assert!(app.files.can_restore());
    assert!(matches!(app.files.status(), Status::Failed(s) if s.contains("exists again")));
}

#[test]
fn the_agent_opens_reveals_and_selects() {
    let mut app = App::with("docs/\n");
    assert_eq!(
        app.files.act_open(&mut app.os, "../etc"),
        Err("That is not a path under your home folder.".into())
    );

    // A folder opens.
    assert_eq!(
        app.files.act_open(&mut app.os, "docs").unwrap(),
        "Opening docs."
    );
    assert_eq!(app.last(), r#"fs_list {path: "docs"}"#);
    app.answer(Ok("a.txt\nb.txt"));
    assert_eq!(app.files.dir(), "docs");

    // A file shows in its folder, selected and previewed.
    app.files.act_open(&mut app.os, "docs/b.txt").unwrap();
    assert_eq!(app.last(), r#"fs_list {path: "docs/b.txt"}"#);
    app.answer(Err("not a folder"));
    assert_eq!(app.last(), r#"fs_list {path: "docs"}"#);
    app.answer(Ok("a.txt\nb.txt"));
    assert_eq!(app.files.selected(), Some("b.txt"));
    assert_eq!(app.last(), r#"fs_read {path: "docs/b.txt"}"#);

    // Reveal shows the folder a path is in.
    app.files.act_reveal(&mut app.os, "docs/a.txt").unwrap();
    app.answer(Ok("a.txt\nb.txt"));
    assert_eq!(app.files.selected(), Some("a.txt"));
    assert!(app.files.act_reveal(&mut app.os, "").is_err());

    // Select takes a name in this folder only.
    assert_eq!(
        app.files.act_select(&mut app.os, "b.txt").unwrap(),
        "Selected b.txt."
    );
    assert!(app.files.act_select(&mut app.os, "nope.txt").is_err());
    assert!(app.files.act_select(&mut app.os, "../b.txt").is_err());

    // Home is `""`.
    assert_eq!(
        app.files.act_open(&mut app.os, "").unwrap(),
        "Opening Home."
    );
}

#[test]
fn a_path_too_long_to_ask_about_fails_in_words() {
    // A 255-byte folder name is a fine path, but nothing can be asked inside it.
    let long = "d".repeat(255);
    let mut app = App::with(&format!("{long}/\n"));
    app.files.select(&mut app.os, &long).unwrap();
    app.answer(Ok("child/\nchild.txt"));
    app.files.open_selected(&mut app.os);
    app.answer(Ok("child/\nchild.txt"));
    assert_eq!(app.files.dir(), long);

    let before = app.os.calls.len();
    app.files.select(&mut app.os, "child.txt").unwrap();
    assert_eq!(app.os.calls.len(), before);
    assert!(matches!(app.files.status(), Status::Failed(_)));
    assert!(matches!(app.files.preview(), Preview::Failed(_)));
    app.files.trash_selected(&mut app.os);
    assert_eq!(app.os.calls.len(), before);
}

#[test]
fn a_window_that_asks_too_much_forgets_the_oldest_question() {
    let mut app = App::with("a.txt\n");
    for _ in 0..40 {
        app.files.refresh(&mut app.os);
    }
    // The first refresh's ticket is long forgotten.
    assert!(!app.answer_to(2, Ok("x.txt")));
}

#[test]
fn hostile_listings_never_panic_and_leave_only_usable_names() {
    let sample = b"docs/\nnotes.txt\nold/\n.hidden\nMy File.md\n".to_vec();
    let mut rng = hostile::XorShift::new(0x5eed);
    for _ in 0..3_000 {
        let bytes = hostile::mutate(&mut rng, &sample);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut app = App::with(&text);
        let items = app.files.items();
        assert!(items.len() <= MAX_ITEMS);
        for (i, item) in items.iter().enumerate() {
            assert!(name_ok(&item.name), "{:?}", item.name);
            assert!(!items[..i].iter().any(|o| o.name == item.name));
        }
        // Whatever it listed can be selected, previewed and trashed by name.
        if let Some(first) = app.files.items().first().map(|i| i.name.clone()) {
            app.files.select(&mut app.os, &first).unwrap();
            app.answer(Ok(&text));
            app.files.trash_selected(&mut app.os);
            assert!(app.last().starts_with("fs_"));
        }
    }
}

#[test]
fn hostile_text_and_messages_are_tamed_before_they_are_shown() {
    let mut app = App::with("a.txt\n");
    app.files.select(&mut app.os, "a.txt").unwrap();
    app.answer(Ok("clean\n\u{1b}[31mred\u{1b}[0m\ttab\r\nbell\u{7}\0"));
    let Preview::Text { text, cut } = app.files.preview() else {
        panic!("a text preview");
    };
    assert!(!cut);
    assert!(
        text.chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\t')
    );
    assert!(text.starts_with("clean\n\u{fffd}[31mred"));

    // A long file is cut by characters, and by lines.
    app.files.select(&mut app.os, "a.txt").unwrap();
    app.answer(Ok(&"x".repeat(PREVIEW_CHARS * 4)));
    let Preview::Text { text, cut } = app.files.preview() else {
        panic!("a text preview");
    };
    assert!(*cut);
    assert_eq!(text.chars().count(), PREVIEW_CHARS);

    app.files.select(&mut app.os, "a.txt").unwrap();
    app.answer(Ok(&"x\n".repeat(500)));
    let Preview::Text { text, cut } = app.files.preview() else {
        panic!("a text preview");
    };
    assert!(*cut);
    assert_eq!(text.lines().count(), PREVIEW_LINES);

    // A message is one short line, with no escape in it.
    app.files.refresh(&mut app.os);
    app.answer(Err(&format!(
        "\u{1b}[2J{}\nsecond line",
        "boom ".repeat(100)
    )));
    let Status::Failed(why) = app.files.status() else {
        panic!("a failure");
    };
    assert!(why.chars().count() <= MESSAGE_CHARS + 1);
    assert!(why.chars().all(|c| !c.is_control()));
}

#[test]
fn answers_in_any_order_leave_a_consistent_window() {
    // Random answers (the right kind and the wrong kind) to whatever was asked.
    let mut rng = hostile::XorShift::new(0xf11e5);
    let texts = ["", "a.txt\nb/\n", "\n\n", "x/y\n..\n", "ok.txt"];
    let mut app = App::with("a.txt\nb/\n");
    for _ in 0..2_000 {
        match rng.below(9) {
            0 => app.files.refresh(&mut app.os),
            1 => app.files.up(&mut app.os),
            2 => app.files.open_selected(&mut app.os),
            3 => app.files.trash_selected(&mut app.os),
            4 => app.files.restore_last(&mut app.os),
            5 => {
                let names = app.names();
                if !names.is_empty() {
                    let name = names[rng.below(names.len())]
                        .trim_end_matches('/')
                        .to_string();
                    let _ = app.files.select(&mut app.os, &name);
                }
            }
            6 => {
                app.files
                    .set_rename(["", "z", "d/", "q/z", ".."][rng.below(5)]);
                app.files.move_selected(&mut app.os);
            }
            _ => {
                let ticket = rng.below(app.os.calls.len() + 3) as u32;
                let text = texts[rng.below(texts.len())];
                let outcome = if rng.below(4) == 0 {
                    Err(text)
                } else {
                    Ok(text)
                };
                app.answer_to(ticket, outcome);
            }
        }
        // Whatever happened: the selection, if any, is in the list.
        if let Some(name) = app.files.selected() {
            assert!(app.files.items().iter().any(|i| i.name == name) || app.files.busy());
        }
        assert!(app.files.items().len() <= MAX_ITEMS);
    }
}

#[test]
fn a_window_can_come_back_to_a_folder_and_list_it() {
    let mut files = Files::at("docs/old");
    let mut os = comp_files::fake::Queue::default();
    assert_eq!(files.dir(), "docs/old");
    files.refresh(&mut os);
    assert_eq!(os.calls[0].args, r#"{path: "docs/old"}"#);
    // A path that is not one is home.
    assert_eq!(Files::at("../etc").dir(), "");
    assert_eq!(Files::at("/abs").dir(), "");
}
