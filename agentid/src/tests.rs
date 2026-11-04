//! The roster, the door, the marks, the questions and the log's name.

use crate::*;
use agentabi::{ROWS, Tier};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::format;

const NOTES: &str = "kai-app 1\nid notes-agent\nabi 1.0\nagent 1\nclass background\n\
    model tokens/day=50000 kv=1024 pool=64m\nactions fs_list fs_read\n\
    memory read write\nnet none\n";

fn manifest(id: &str) -> kaiagent::Manifest {
    let mut manifest = kaiagent::Manifest::parse(NOTES).unwrap();
    manifest.id = id.to_string();
    manifest
}

/// `NOTES` with its actions replaced.
fn manifest_with(actions: &str) -> kaiagent::Manifest {
    kaiagent::Manifest::parse(&NOTES.replace("fs_list fs_read", actions)).unwrap()
}

fn roster_with_notes() -> (Roster, AgentId) {
    let mut roster = Roster::new();
    let id = roster.install(manifest("notes-agent")).unwrap();
    (roster, id)
}

/// The person's own hardware, for the tests that stand in for the input path.
fn person() -> Source {
    // SAFETY: a test is the input path here.
    unsafe { Source::hardware() }
}

fn find(name: &str) -> agentabi::Action {
    *agentabi::find(&ROWS, name).unwrap()
}

// The roster.

#[test]
fn a_new_roster_has_the_four_built_ins_with_the_names_no_package_may_take() {
    let roster = Roster::new();
    let names: Vec<_> = roster.iter().map(|a| a.name().as_str()).collect();
    assert_eq!(names, kaiagent::BUILTIN);
    let badges: Vec<_> = roster.iter().map(|a| a.id().badge()).collect();
    assert_eq!(badges, [1, 2, 3, 4]);
    for builtin in Builtin::ALL {
        let agent = roster.by_name(builtin.name()).unwrap();
        assert_eq!(agent.id(), builtin.id());
        assert!(Name::new(builtin.name()).is_ok(), "{}", builtin.name());
    }
}

#[test]
fn a_packaged_agent_is_its_manifest_under_a_new_badge() {
    let (roster, id) = roster_with_notes();
    assert_eq!(id.badge(), 5);
    let agent = roster.get(id).unwrap();
    assert_eq!(agent.name().as_str(), "notes-agent");
    let parsed = kaiagent::Manifest::parse(NOTES).unwrap();
    assert_eq!(agent.manifest(), Some(&parsed));
    assert_eq!(agent.budgets().unwrap().tokens_per_day, 50_000);
    assert!(agent.allows("fs_list") && !agent.allows("set_policy"));
    let caller = roster.caller(5).unwrap();
    assert_eq!((caller.id(), caller.name().as_str()), (id, "notes-agent"));
    assert!(!caller.is_kai());
}

#[test]
fn a_name_belongs_to_one_live_agent_and_a_built_ins_never_to_a_package() {
    let (mut roster, _) = roster_with_notes();
    let again = roster.install(manifest("notes-agent"));
    let name = Name::new("notes-agent").unwrap();
    assert_eq!(again, Err(InstallError::Taken(name)));
    // kaiagent refuses a manifest with a built-in's name; the roster does too, for a
    // manifest built by hand.
    for builtin in kaiagent::BUILTIN {
        assert!(matches!(
            roster.install(manifest(builtin)),
            Err(InstallError::Taken(_))
        ));
    }
    assert_eq!(
        roster.install(manifest("Notes")),
        Err(InstallError::BadName)
    );
    assert_eq!(roster.len(), 5);
}

#[test]
fn only_the_roster_first_agent_is_kai_whatever_a_name_says() {
    let (roster, _) = roster_with_notes();
    let kai = roster.caller(1).unwrap();
    assert!(kai.is_kai() && kai.name().is_kai());
    for badge in 2..=5 {
        assert!(!roster.caller(badge).unwrap().is_kai(), "{badge}");
    }
}

#[test]
fn a_badge_is_never_handed_out_twice_even_after_a_retire() {
    let (mut roster, first) = roster_with_notes();
    let old = roster.caller(first.badge()).unwrap();
    roster.retire(first).unwrap();
    assert!(roster.caller(first.badge()).is_err());
    let second = roster.install(manifest("notes-agent")).unwrap();
    assert!(second > first, "{second:?} {first:?}");
    // The retired agent's caller does not pass for the new agent of the same name.
    assert!(!roster.is_live(&old));
    assert!(roster.is_live(&roster.caller(second.badge()).unwrap()));
}

#[test]
fn a_caller_is_live_only_on_the_roster_that_made_it() {
    let (one, id) = roster_with_notes();
    let (two, _) = roster_with_notes();
    let caller = one.caller(id.badge()).unwrap();
    assert!(one.is_live(&caller) && !two.is_live(&caller));
    assert_eq!(two.admit(&caller, "fs_list"), Err(Denied::Retired));
    // A copy of a roster is the same roster.
    assert!(one.clone().is_live(&caller));
}

#[test]
fn the_built_ins_stay_and_unknown_badges_are_refused() {
    let mut roster = Roster::new();
    for builtin in Builtin::ALL {
        let id = roster.by_name(builtin.name()).unwrap().id();
        assert_eq!(roster.retire(id).unwrap_err(), InstallError::NotPackaged);
        assert_eq!(
            roster.upgrade(id, manifest("kai")),
            Err(InstallError::NotPackaged)
        );
    }
    assert_eq!(roster.len(), 4);
    for badge in [0, 5, u32::MAX] {
        assert_eq!(roster.caller(badge), Err(UnknownBadge(badge)));
    }
}

#[test]
fn an_upgrade_changes_the_ceiling_and_keeps_the_identity() {
    let (mut roster, id) = roster_with_notes();
    let caller = roster.caller(id.badge()).unwrap();
    assert!(roster.admit(&caller, "set_policy").is_err());
    let mut wider = manifest("notes-agent");
    wider.actions = manifest_with("set_policy").actions;
    roster.upgrade(id, wider).unwrap();
    assert!(roster.is_live(&caller));
    assert!(roster.admit(&caller, "set_policy").is_ok());
    assert_eq!(
        roster.upgrade(id, manifest("other-agent")),
        Err(InstallError::NameChanged)
    );
    let gone = roster.install(manifest("gone-agent")).unwrap();
    roster.retire(gone).unwrap();
    assert_eq!(
        roster.upgrade(gone, manifest("gone-agent")),
        Err(InstallError::NotPackaged)
    );
}

#[test]
fn only_so_many_packaged_agents_install() {
    let mut roster = Roster::new();
    for n in 0..MAX_PACKAGED {
        roster.install(manifest(&format!("ag{n:02}"))).unwrap();
    }
    assert_eq!(
        roster.install(manifest("one-more")),
        Err(InstallError::Full)
    );
    // A retire makes room, and the new agent still gets a badge nobody had.
    let gone = roster.by_name("ag00").unwrap().id();
    roster.retire(gone).unwrap();
    let fresh = roster.install(manifest("one-more")).unwrap();
    assert!(fresh.badge() > 4 + MAX_PACKAGED as u32);
}

// Names.

#[test]
fn a_name_has_the_packages_shape_and_cannot_split_a_record() {
    for good in ["kai", "ab", "notes-agent", "a1-2", "abcdefghijkl"] {
        assert_eq!(Name::new(good).unwrap().as_str(), good);
    }
    for bad in [
        "",
        "a",
        "Kai",
        "1ab",
        "-ab",
        "a b",
        "a=b",
        "a\nb",
        "abcdefghijklm",
        "n\u{e9}",
        "a_b",
    ] {
        assert_eq!(Name::new(bad), Err(BadName), "{bad:?}");
    }
    assert!(Name::kai().is_kai() && !Name::new("goals").unwrap().is_kai());
}

// The door.

#[test]
fn kai_may_call_any_row_and_a_packaged_agent_only_what_its_manifest_names() {
    let (roster, id) = roster_with_notes();
    let kai = roster.caller(1).unwrap();
    for row in ROWS.iter().filter(|r| agentabi::derive(r) != Tier::Never) {
        assert!(roster.admit(&kai, row.name).is_ok(), "{}", row.name);
    }
    let notes = roster.caller(id.badge()).unwrap();
    let admitted = roster.admit(&notes, "fs_list").unwrap();
    assert_eq!((admitted.row(), admitted.caller()), ("fs_list", &notes));
    assert_eq!(
        roster.admit(&notes, "set_policy"),
        Err(Denied::OutsideGrants)
    );
    assert_eq!(roster.admit(&notes, "fs_write"), Err(Denied::OutsideGrants));
}

#[test]
fn goals_index_and_maint_hold_nothing_until_their_rows_exist() {
    let roster = Roster::new();
    for badge in [2, 3, 4] {
        let caller = roster.caller(badge).unwrap();
        assert_eq!(roster.admit(&caller, "fs_list"), Err(Denied::OutsideGrants));
    }
}

#[test]
fn an_unknown_row_a_never_row_and_a_retired_agent_are_refused_before_anything_else() {
    let (mut roster, id) = roster_with_notes();
    let kai = roster.caller(1).unwrap();
    assert_eq!(roster.admit(&kai, "no_such_row"), Err(Denied::UnknownRow));
    let mut never = find("fs_list");
    never.min_tier = Tier::Never;
    assert_eq!(
        roster.admit_in(&kai, "fs_list", &[never]),
        Err(Denied::Never)
    );
    let notes = roster.caller(id.badge()).unwrap();
    roster.retire(id).unwrap();
    assert_eq!(roster.admit(&notes, "fs_list"), Err(Denied::Retired));
}

#[test]
fn an_installed_apps_rows_are_admitted_against_the_registry_the_door_is_given() {
    let (roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let app_row = find("fs_list");
    assert!(roster.admit_in(&notes, "fs_list", &[app_row]).is_ok());
    assert_eq!(
        roster.admit_in(&notes, "fs_read", &[app_row]),
        Err(Denied::UnknownRow)
    );
}

#[test]
fn a_row_asked_for_by_its_old_name_is_admitted_as_the_new_one() {
    let (roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let mut renamed = find("fs_list");
    renamed.old_name = Some("fs_ls");
    let admitted = roster.admit_in(&notes, "fs_ls", &[renamed]).unwrap();
    assert_eq!(admitted.row(), "fs_list");
}

#[test]
fn an_admission_stamps_its_entry_with_the_callers_name() {
    let (roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let stamp = roster.admit(&notes, "fs_read").unwrap().stamp();
    assert_eq!(stamp.field(), "agent=notes-agent");
    let kai = roster.caller(1).unwrap();
    let kai = roster.admit(&kai, "cpus").unwrap().stamp();
    assert_eq!(kai.field(), "agent=kai");
}

#[test]
fn an_admission_held_past_a_retire_or_an_upgrade_no_longer_holds() {
    let (mut roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let held = roster.admit(&notes, "fs_list").unwrap();
    assert_eq!(roster.revalidate(&held), Ok(()));
    // An upgrade that takes the row away.
    let mut narrower = manifest("notes-agent");
    narrower.actions = manifest_with("fs_read").actions;
    roster.upgrade(id, narrower).unwrap();
    assert_eq!(roster.revalidate(&held), Err(Denied::OutsideGrants));
    // A retire.
    let held = roster.admit(&notes, "fs_read").unwrap();
    roster.retire(id).unwrap();
    assert_eq!(roster.revalidate(&held), Err(Denied::Retired));
    // Not even a later agent of the same name.
    roster.install(manifest("notes-agent")).unwrap();
    assert_eq!(roster.revalidate(&held), Err(Denied::Retired));
}

// Marks and consent words.

#[test]
fn only_hardware_makes_a_persons_mark() {
    assert!(Mark::of(person()).is_person());
    assert_eq!(Mark::of(person()).agent(), None);
    let mut roster = Roster::new();
    for n in 0..MAX_PACKAGED {
        roster.install(manifest(&format!("ag{n:02}"))).unwrap();
    }
    for agent in roster.iter() {
        let mark = Mark::of(Source::agent(agent.id()));
        assert!(!mark.is_person(), "{}", agent.name());
        assert_eq!(mark.agent(), Some(agent.id()));
    }
    let kai = roster.by_name("kai").unwrap().id();
    let line = Line::new("yes", Source::agent(kai));
    assert!(!line.mark().is_person() && line.text() == "yes");
    assert!(Line::new("yes", person()).mark().is_person());
}

const WORDS: &str = "switch to round robin, yes, y, answer 1 y";

/// A question as shown, for the tests of questions.
const SHOWN: &str = "fs_list(\"old.log\")";

#[test]
fn consent_words_go_only_to_kai_and_only_from_a_person() {
    let (mut roster, id) = roster_with_notes();
    let typed = Line::new(WORDS, person());
    let kai = roster.caller(1).unwrap();
    let notes = roster.caller(id.badge()).unwrap();
    // The person's line to the person's agent.
    let door = roster.admit(&kai, "set_policy").unwrap();
    assert_eq!(door.consent_words(&typed), WORDS);
    // Text an agent sent to the shell is not the person's, nor are keys it made.
    let sent = Line::new(WORDS, Source::agent(notes.id()));
    assert_eq!(door.consent_words(&sent), "");
    let sent_as_kai = Line::new(WORDS, Source::agent(kai.id()));
    assert_eq!(door.consent_words(&sent_as_kai), "");
    // An agent's own call gets nothing from a person's line, however it was typed.
    let mut wide = manifest("notes-agent");
    wide.actions = manifest_with("set_policy").actions;
    roster.upgrade(id, wide).unwrap();
    let theirs = roster.admit(&notes, "set_policy").unwrap();
    assert_eq!(theirs.consent_words(&typed), "");
    assert_eq!(theirs.consent_words(&sent), "");
}

// Questions.

#[test]
fn only_the_persons_key_answers_and_only_for_the_agent_and_row_that_asked() {
    let (roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let kai = roster.caller(1).unwrap();
    let list = roster.admit(&notes, "fs_list").unwrap();
    let read = roster.admit(&notes, "fs_read").unwrap();
    let mut asks = Asks::new();
    asks.ask(&roster, 7, &list, SHOWN).unwrap();
    assert!(asks.is_open(7) && asks.len() == 1);
    let yes = asks.answer(7, true, Mark::of(person())).unwrap();
    assert_eq!(yes.entry(), 7);
    assert!(yes.yes() && asks.is_empty());
    assert!(yes.spend(&roster, &list, 7, SHOWN));
    // The same yes is not good for another row, nor for another agent's call.
    for (entry, other) in [(8, &read), (9, &roster.admit(&kai, "fs_list").unwrap())] {
        asks.ask(&roster, entry, &list, SHOWN).unwrap();
        let yes = asks.answer(entry, true, Mark::of(person())).unwrap();
        assert!(!yes.spend(&roster, other, entry, SHOWN), "{entry}");
    }
    // A no runs nothing.
    asks.ask(&roster, 10, &list, SHOWN).unwrap();
    let no = asks.answer(10, false, Mark::of(person())).unwrap();
    assert!(!no.yes() && !no.spend(&roster, &list, 10, SHOWN));
}

#[test]
fn an_agent_cannot_answer_a_question_and_learns_nothing_by_trying() {
    let (roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let door = roster.admit(&notes, "fs_list").unwrap();
    let mut asks = Asks::new();
    asks.ask(&roster, 7, &door, SHOWN).unwrap();
    for id in [notes.id(), Builtin::Kai.id()] {
        for yes in [true, false] {
            for entry in [7, 8] {
                let mark = Mark::of(Source::agent(id));
                let got = asks.answer(entry, yes, mark);
                assert_eq!(got, Err(AnswerError::NotThePerson));
            }
        }
    }
    assert!(asks.is_open(7));
    let nothing = asks.answer(8, true, Mark::of(person()));
    assert_eq!(nothing, Err(AnswerError::NothingAsked));
    let yes = asks.answer(7, true, Mark::of(person())).unwrap();
    assert!(yes.spend(&roster, &door, 7, SHOWN));
    // Answered once.
    let twice = asks.answer(7, true, Mark::of(person()));
    assert_eq!(twice, Err(AnswerError::NothingAsked));
}

#[test]
fn a_yes_for_an_agent_retired_since_runs_nothing_and_it_cannot_ask_again() {
    let (mut roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let door = roster.admit(&notes, "fs_list").unwrap();
    let mut asks = Asks::new();
    asks.ask(&roster, 7, &door, SHOWN).unwrap();
    roster.retire(id).unwrap();
    // Neither the question's answer nor a later agent of the same name revives it.
    let yes = asks.answer(7, true, Mark::of(person())).unwrap();
    roster.install(manifest("notes-agent")).unwrap();
    assert!(!yes.spend(&roster, &door, 7, SHOWN));
    assert_eq!(asks.ask(&roster, 8, &door, SHOWN), Err(AskError::Retired));
    // A grant taken away by an upgrade stops a yes the same way.
    let (mut roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let door = roster.admit(&notes, "fs_list").unwrap();
    asks.ask(&roster, 9, &door, SHOWN).unwrap();
    let mut narrower = manifest("notes-agent");
    narrower.actions = manifest_with("fs_read").actions;
    roster.upgrade(id, narrower).unwrap();
    let yes = asks.answer(9, true, Mark::of(person())).unwrap();
    assert!(!yes.spend(&roster, &door, 9, SHOWN));
}

#[test]
fn questions_are_unique_bounded_per_agent_and_leave_room_for_kai() {
    let mut roster = Roster::new();
    let doors: Vec<Admission> = ["ag-a", "ag-b", "ag-c", "ag-d"]
        .iter()
        .map(|name| {
            let id = roster.install(manifest(name)).unwrap();
            let caller = roster.caller(id.badge()).unwrap();
            roster.admit(&caller, "fs_list").unwrap()
        })
        .collect();
    let kai = roster.admit(&roster.caller(1).unwrap(), "cpus").unwrap();
    let mut asks = Asks::new();
    let mut entry = 0u64;
    let mut next = || {
        entry += 1;
        entry
    };
    // Three agents fill their own shares, which is all agents together may hold.
    for door in &doors[..3] {
        for _ in 0..asks::MAX_PER_AGENT {
            asks.ask(&roster, next(), door, SHOWN).unwrap();
        }
        assert_eq!(
            asks.ask(&roster, next(), door, SHOWN),
            Err(AskError::TooMany)
        );
    }
    assert_eq!(asks.len(), asks::MAX_OPEN - asks::KAI_ROOM);
    assert_eq!(
        asks.ask(&roster, next(), &doors[3], SHOWN),
        Err(AskError::Full)
    );
    // kai still gets asked, up to the whole book.
    for _ in 0..asks::KAI_ROOM {
        asks.ask(&roster, next(), &kai, SHOWN).unwrap();
    }
    assert_eq!(asks.ask(&roster, next(), &kai, SHOWN), Err(AskError::Full));
    assert_eq!(asks.len(), asks::MAX_OPEN);
    // An entry asks once.
    assert_eq!(asks.ask(&roster, 1, &kai, SHOWN), Err(AskError::Duplicate));
    // Withdrawing makes room.
    assert!(asks.withdraw(1) && !asks.withdraw(1));
    asks.withdraw_agent(doors[1].caller().id());
    assert_eq!(asks.len(), asks::MAX_OPEN - 1 - asks::MAX_PER_AGENT);
    assert!(!asks.is_open(6));
}

// The log.

#[test]
fn a_field_reads_back_as_the_name_and_a_record_without_one_is_kai() {
    assert_eq!(read_field(Some("agent=goals")).unwrap().as_str(), "goals");
    assert_eq!(read_field(None).unwrap(), Name::kai());
    for (word, why) in [
        ("owner=goals", FieldError::NotAgentField),
        ("agentgoals", FieldError::NotAgentField),
        ("agent", FieldError::NotAgentField),
        ("agent=", FieldError::BadName),
        ("agent=Goals", FieldError::BadName),
        ("agent=a b", FieldError::BadName),
        ("agent=x", FieldError::BadName),
        ("agent=goals=kai", FieldError::BadName),
    ] {
        assert_eq!(read_field(Some(word)), Err(why), "{word:?}");
    }
    let (roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let written = roster.admit(&notes, "fs_list").unwrap().stamp().field();
    assert_eq!(read_field(Some(&written)).unwrap(), *notes.name());
}

#[test]
fn undo_of_an_agent_picks_its_newest_live_entry_and_only_its_own_may_be_undone() {
    let (roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let kai = roster.caller(1).unwrap();
    let (n, k) = (notes.name().clone(), kai.name().clone());
    let log = [(3, &n, true), (5, &k, true), (6, &n, false), (4, &n, true)];
    assert_eq!(last_live(log, "notes-agent"), Some(4));
    assert_eq!(last_live(log, "kai"), Some(5));
    assert_eq!(last_live(log, "goals"), None);
    assert!(kai.may_undo(&n) && kai.may_undo(&k));
    assert!(notes.may_undo(&n) && !notes.may_undo(&k));
}

#[test]
fn errors_say_what_went_wrong_in_words() {
    let name = Name::new("notes-agent").unwrap();
    let said: Vec<String> = Vec::from([
        InstallError::Taken(name).to_string(),
        InstallError::Full.to_string(),
        InstallError::BadName.to_string(),
        InstallError::NotPackaged.to_string(),
        InstallError::NameChanged.to_string(),
        UnknownBadge(9).to_string(),
        BadName.to_string(),
        Denied::Retired.to_string(),
        Denied::UnknownRow.to_string(),
        Denied::Never.to_string(),
        Denied::OutsideGrants.to_string(),
    ]);
    assert!(said.iter().all(|s| !s.is_empty()));
    assert!(said[0].contains("notes-agent") && said[5].contains('9'));
}

#[test]
fn a_yes_is_for_the_call_it_answered_and_not_the_next_one_that_looks_like_it() {
    let (roster, id) = roster_with_notes();
    let notes = roster.caller(id.badge()).unwrap();
    let door = roster.admit(&notes, "fs_list").unwrap();
    let mut asks = Asks::new();
    let (old, thesis) = ("fs_list(\"old.log\")", "fs_list(\"thesis.txt\")");
    let answer = |asks: &mut Asks, entry| asks.answer(entry, true, Mark::of(person())).unwrap();
    // The same agent, row and entry, other arguments.
    asks.ask(&roster, 7, &door, old).unwrap();
    assert!(!answer(&mut asks, 7).spend(&roster, &door, 7, thesis));
    // The same words, another entry (a later request that shows the same question).
    asks.ask(&roster, 8, &door, old).unwrap();
    assert!(!answer(&mut asks, 8).spend(&roster, &door, 9, old));
    // Only the call it answered.
    asks.ask(&roster, 10, &door, old).unwrap();
    assert!(answer(&mut asks, 10).spend(&roster, &door, 10, old));
}

#[test]
fn a_digest_tells_the_entry_the_row_and_the_words_apart() {
    use crate::digest::Digest;
    let base = Digest::of(1, "fs_list", "x");
    assert_eq!(base, Digest::of(1, "fs_list", "x"));
    assert_ne!(base, Digest::of(2, "fs_list", "x"));
    assert_ne!(base, Digest::of(1, "fs_read", "x"));
    assert_ne!(base, Digest::of(1, "fs_list", "y"));
    // Length-prefixed: text moved from one field to the other is another digest.
    assert_ne!(Digest::of(1, "ab", "c"), Digest::of(1, "a", "bc"));
    assert_ne!(Digest::of(1, "", "abc"), Digest::of(1, "abc", ""));
}
