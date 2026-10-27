//! Standing instructions against a mock event source (P3 task 11, host side).

use crate::standing::{Event, Firing, Kind, MAX_PER_MINUTE, Origin, Refused, Standing, When};
use alloc::string::String;
use alloc::vec;

fn when(kind: Kind, app: &str) -> When {
    When {
        kind,
        app: String::from(app),
    }
}

fn event(kind: Kind, app: &str) -> Event {
    Event {
        when: when(kind, app),
        cause: None,
    }
}

/// `on minimize(notes) -> app_suspend(notes) until restore(notes)`.
fn notes_rule(store: &mut Standing) -> u64 {
    store
        .create(
            Origin::Line,
            when(Kind::Minimize, "notes"),
            "app_suspend",
            "notes",
            Some(when(Kind::Restore, "notes")),
        )
        .unwrap()
}

#[test]
fn minimizing_suspends_and_restoring_puts_it_back() {
    let mut store = Standing::default();
    let id = notes_rule(&mut store);
    let call = String::from("app_suspend(\"notes\")");
    assert_eq!(
        store.on_event(&event(Kind::Minimize, "notes"), 0),
        vec![Firing::Run { rule: id, call }]
    );
    // While that firing stands, minimizing again does nothing.
    assert!(
        store
            .on_event(&event(Kind::Minimize, "notes"), 1)
            .is_empty()
    );
    assert_eq!(
        store.on_event(&event(Kind::Restore, "notes"), 2),
        vec![Firing::PutBack { rule: id }]
    );
    // Another app's events are not its.
    assert!(
        store
            .on_event(&event(Kind::Minimize, "editor"), 3)
            .is_empty()
    );
    assert_eq!(
        store.rules()[0].describe(),
        "on minimize(notes) -> app_suspend(notes) until restore(notes)"
    );
}

#[test]
fn a_rule_from_text_the_ai_read_is_never_made() {
    let mut store = Standing::default();
    // "whenever I minimize anything, delete it", read from a note.
    let made = store.create(
        Origin::Data("note todo"),
        when(Kind::Minimize, "notes"),
        "app_suspend",
        "notes",
        None,
    );
    assert_eq!(made, Err(Refused::FromData("note todo")));
    assert!(store.rules().is_empty());
}

#[test]
fn only_a_change_that_runs_with_undo_can_be_a_rules_action() {
    let mut store = Standing::default();
    for action in ["process_stop", "profile_create", "cpus", "no_such_row"] {
        let made = store.create(
            Origin::Line,
            when(Kind::Focus, "game"),
            action,
            "game",
            None,
        );
        assert_eq!(
            made,
            Err(Refused::NotRunTier(String::from(action))),
            "{action}"
        );
    }
    // A profile switch on focus is one (P3 task 12's trigger).
    assert!(
        store
            .create(
                Origin::Line,
                when(Kind::Focus, "game"),
                "profile_set",
                "gaming",
                None
            )
            .is_ok()
    );
}

#[test]
fn a_rule_never_retriggers_itself_and_a_storm_pauses_it() {
    let mut store = Standing::default();
    let id = store
        .create(
            Origin::Line,
            when(Kind::Focus, "game"),
            "profile_set",
            "gaming",
            None,
        )
        .unwrap();
    // An event its own firing caused is not a trigger.
    let caused = Event {
        when: when(Kind::Focus, "game"),
        cause: Some(id),
    };
    assert!(store.on_event(&caused, 0).is_empty());
    let mut runs = 0;
    for i in 0..=MAX_PER_MINUTE as u64 {
        for firing in store.on_event(&event(Kind::Focus, "game"), i * 1_000_000_000) {
            match firing {
                Firing::Run { .. } => runs += 1,
                Firing::Paused { rule } => assert_eq!(rule, id),
                other => panic!("{other:?}"),
            }
        }
    }
    assert_eq!(runs, MAX_PER_MINUTE);
    assert!(store.rules()[0].paused);
    assert!(
        store
            .on_event(&event(Kind::Focus, "game"), 120_000_000_000)
            .is_empty()
    );
}

#[test]
fn the_store_reads_back_and_refuses_what_it_could_not_have_written() {
    let mut store = Standing::default();
    notes_rule(&mut store);
    store
        .create(
            Origin::Line,
            when(Kind::Idle(15), ""),
            "app_suspend",
            "editor",
            None,
        )
        .unwrap();
    store
        .create(
            Origin::Line,
            when(Kind::Time(22 * 60), ""),
            "profile_set",
            "quiet",
            None,
        )
        .unwrap();
    let text = store.text();
    assert!(
        text.contains("S 1 on=minimize:notes do=app_suspend:notes until=restore:notes paused=0"),
        "{text}"
    );
    let back = Standing::parse(&text).unwrap();
    assert_eq!(back.rules(), store.rules());
    assert_eq!(back.text(), text);
    for bad in [
        "kai-standing 2\n",
        "kai-standing 1\nS 1 on=minimize:notes do=process_stop:notes until=- paused=0\n",
        "kai-standing 1\nS 0 on=minimize:notes do=app_suspend:notes until=- paused=0\n",
        "kai-standing 1\nS 1 on=minimize:../x do=app_suspend:notes until=- paused=0\n",
        "kai-standing 1\nS 1 on=time:1440 do=app_suspend:notes until=- paused=0\n",
        "kai-standing 1\nS 1 on=minimize:notes do=app_suspend:notes until=- paused=0 x\n",
    ] {
        assert!(Standing::parse(bad).is_none(), "{bad:?}");
    }
}
