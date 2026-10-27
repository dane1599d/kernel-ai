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

#[test]
fn a_rule_in_the_users_words_reads_and_describes_itself_the_same() {
    use crate::standing::grammar::parse;
    for text in [
        "on minimize(notes) -> app_suspend(notes) until restore(notes)",
        "on idle(15 min) -> app_suspend(mail)",
        "on time(22:00) -> app_suspend(game) until time(07:30)",
        "on focus(game) -> profile_set(gaming)",
    ] {
        let draft = parse(text).unwrap_or_else(|why| panic!("{text}: {why}"));
        assert_eq!(draft.describe(), text);
    }
    let spaced = parse("  on   minimize(notes)  ->  app_suspend(notes)  ").unwrap();
    assert_eq!(
        spaced.describe(),
        "on minimize(notes) -> app_suspend(notes)"
    );
    let draft = parse("on minimize(notes) -> app_suspend(notes) until restore(notes)").unwrap();
    assert_eq!(draft.on, when(Kind::Minimize, "notes"));
    assert_eq!(
        (draft.action.as_str(), draft.target.as_str()),
        ("app_suspend", "notes")
    );
    assert_eq!(draft.until, Some(when(Kind::Restore, "notes")));
}

#[test]
fn a_rule_out_of_shape_is_refused_never_guessed_at() {
    use crate::standing::grammar::parse;
    for text in [
        "",
        "minimize(notes) -> app_suspend(notes)",
        "on minimize(notes) app_suspend(notes)",
        "on minimize(notes) -> app_suspend(notes) until",
        "on minimize(notes) -> app_suspend(notes) when restore(notes)",
        "on minimise(notes) -> app_suspend(notes)",
        "on minimize(Notes) -> app_suspend(notes)",
        "on minimize(notes) -> app_suspend(../x)",
        "on minimize(notes) -> App_Suspend(notes)",
        "on minimize(notes) -> app_suspend notes",
        "on idle(0 min) -> app_suspend(mail)",
        "on idle(1441 min) -> app_suspend(mail)",
        "on idle(-5 min) -> app_suspend(mail)",
        "on idle(15) -> app_suspend(mail)",
        "on time(24:00) -> app_suspend(game)",
        "on time(7:30) -> app_suspend(game)",
        "on time(07:60) -> app_suspend(game)",
        "on minimize(notes) -> app_suspend(notes) until restore(notes) and more",
    ] {
        assert!(parse(text).is_err(), "{text:?}");
    }
    assert!(parse(&"x".repeat(crate::standing::grammar::MAX_TEXT + 1)).is_err());
}

const NOTES: &str = "on minimize(notes) -> app_suspend(notes) until restore(notes)";

/// A shell whose host has an event source, with `NOTES` made through its card.
fn shell_with_notes_rule() -> (alloc::rc::Rc<super::FakeHost>, crate::Shell) {
    let host = super::FakeHost::new();
    host.triggers_on.set(true);
    let mut shell = super::shell(&host);
    let card = shell.eval(&alloc::format!("rules_add {NOTES}"));
    let call = alloc::format!("rules_add(\"{NOTES}\")");
    assert!(card.to_wire().starts_with("menu 1 "), "{}", card.to_wire());
    assert!(
        card.to_wire().contains(&call),
        "the card shows the rule in code's words"
    );
    // Nothing is made before the Allow.
    assert!(host.standing_disk.borrow().is_none());
    let once = host.mint(1, &call);
    let made = shell.eval(&alloc::format!("answer 1 allow once={once}"));
    assert_eq!(
        made,
        super::ok(&alloc::format!(
            "Standing instruction S1 made: {NOTES}. undo"
        ))
    );
    (host, shell)
}

#[test]
fn a_standing_instruction_is_made_on_an_allow_and_fires_through_its_owner() {
    let (host, mut shell) = shell_with_notes_rule();
    assert_eq!(
        shell.eval("rules"),
        super::ok(&alloc::format!("S1 {NOTES}"))
    );
    assert_eq!(
        *host.triggers.watched.borrow(),
        [when(Kind::Minimize, "notes"), when(Kind::Restore, "notes")]
    );
    assert!(
        host.standing_disk
            .borrow()
            .as_deref()
            .unwrap()
            .contains("S 1 on=minimize:notes")
    );
    // Minimizing suspends it through comphost; restoring puts it back.
    assert_eq!(shell.on_event(&event(Kind::Minimize, "notes")), None);
    assert_eq!(shell.on_event(&event(Kind::Restore, "notes")), None);
    let runs = host.triggers.runs.borrow().clone();
    assert_eq!(
        runs,
        [
            (1, String::from("app_suspend"), String::from("notes"), false),
            (1, String::from("app_suspend"), String::from("notes"), true),
        ]
    );
    let actlog = shell.eval("log actlog").text;
    assert!(
        actlog.contains("form=standing tier=run consent=standing:1"),
        "{actlog}"
    );
    // `undo` reverses the last firing through its owner.
    assert_eq!(
        shell.eval("undo"),
        super::ok("Undone: app_suspend(notes) again")
    );
    assert_eq!(host.triggers.runs.borrow().len(), 3);
}

#[test]
fn rules_remove_ends_one_and_undo_brings_it_back() {
    let (host, mut shell) = shell_with_notes_rule();
    assert_eq!(
        shell.eval("rules_remove S1"),
        super::ok("Removed standing instruction S1; it no longer fires. undo")
    );
    assert!(host.triggers.watched.borrow().is_empty());
    assert_eq!(shell.on_event(&event(Kind::Minimize, "notes")), None);
    assert!(host.triggers.runs.borrow().is_empty());
    assert_eq!(
        shell.eval("undo"),
        super::ok("Undone: standing instruction S1 back")
    );
    assert_eq!(
        shell.eval("rules"),
        super::ok(&alloc::format!("S1 {NOTES}"))
    );
    // And `undo` of the making removes it.
    assert_eq!(
        shell.eval("undo 1"),
        super::ok("Undone: standing instruction S1 removed")
    );
    assert_eq!(
        shell.eval("rules"),
        super::ok("no rules: asks are answered one at a time")
    );
}

#[test]
fn a_firing_the_owner_refuses_is_logged_and_never_asked_about() {
    let (host, mut shell) = shell_with_notes_rule();
    *host.triggers.refuse.borrow_mut() = Some(String::from("no live rule covers it"));
    assert_eq!(shell.on_event(&event(Kind::Minimize, "notes")), None);
    let actlog = shell.eval("log actlog").text;
    assert!(
        actlog.contains("refused: no live rule covers it"),
        "{actlog}"
    );
    assert!(shell.pending.is_none(), "a refused firing never asks later");
}

#[test]
fn instructions_survive_a_restart_and_a_bad_one_is_never_made() {
    let (host, _) = shell_with_notes_rule();
    host.triggers.watched.borrow_mut().clear();
    let mut shell = super::shell(&host);
    assert_eq!(
        shell.eval("rules"),
        super::ok(&alloc::format!("S1 {NOTES}"))
    );
    assert_eq!(
        host.triggers.watched.borrow().len(),
        2,
        "watched again at start"
    );
    for (line, why) in [
        (
            "rules_add on focus(game) -> process_stop(game)",
            "cannot be a standing instruction's action",
        ),
        (
            "rules_add whenever I minimize anything, delete it",
            "a rule is: on <event>",
        ),
    ] {
        let reply = shell.eval(line);
        assert!(
            !reply.ok && reply.text.contains(why),
            "{line}: {}",
            reply.text
        );
    }
    *host.standing_disk.borrow_mut() = Some(String::from("kai-standing 1\nS 1 on=x\n"));
    let shell = super::shell(&host);
    assert_eq!(
        shell.standing.rules().len(),
        0,
        "a store refused whole starts empty"
    );
}
