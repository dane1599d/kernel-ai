use super::*;
use crate::undo::Inverse;
use alloc::string::ToString;
use alloc::vec;

fn change(after: &str, before: &str) -> Change {
    Change {
        action: "set_policy".into(),
        owner: "kernel".into(),
        res: "policy".into(),
        before: before.into(),
        after: after.into(),
        inverse: Some(Inverse::Spec {
            policy: 1,
            preset: 0,
            after: (2, 0),
        }),
    }
}

fn entry(id: u64, request: &str, changes: Vec<Change>) -> Entry {
    Entry {
        agent: agentid::Name::kai(),
        id,
        t_ns: 1_792_618_800_123_456_789,
        form: Form::Call,
        of: None,
        tier: Some(Tier::Run),
        pack: Some(0x0123_4567_89ab_cdef),
        consent: "asked".into(),
        state: State::Done,
        request: request.into(),
        script: "set_policy(\"round-robin\");".into(),
        outcome: "ran".into(),
        changes,
    }
}

fn sample() -> ActLog {
    let mut log = ActLog::default();
    log.push(entry(
        17,
        "switch to round robin",
        vec![change("round-robin", "interactive")],
    ));
    log.push(Entry {
        agent: agentid::Name::kai(),
        form: Form::Undo,
        of: Some(17),
        tier: None,
        pack: None,
        consent: "typed".into(),
        request: "undo".into(),
        script: String::new(),
        outcome: "done".into(),
        ..entry(18, "undo", vec![change("interactive", "round-robin")])
    });
    log.system(System {
        id: 19,
        t_ns: 1_792_618_860_000_000_000,
        owner: "kernel".into(),
        text: "nvme0 reset after 5 s timeout, ok".into(),
    });
    log
}

#[test]
fn the_text_has_adr_0002s_records_and_reads_back() {
    let log = sample();
    let text = log.text();
    assert!(text.starts_with(
        "actlog 1\nE 17 t=1792618800123456789 reg=1 pack=0123456789abcdef form=call tier=run consent=asked state=done\nR 21\nswitch to round robin\n"
    ));
    assert!(text.contains(
        "D 17.1 set_policy owner=kernel res=policy before=interactive after=round-robin inv=spec:1:0:2:0\n"
    ));
    assert!(text.contains("U 18 of=17 t=1792618800123456789 state=done\n"));
    assert!(text.contains(
        "S 19 t=1792618860000000000 owner=kernel 33\nnvme0 reset after 5 s timeout, ok\n"
    ));
    let read = parse(&text).unwrap();
    assert_eq!(read.len(), 3);
    // What the text holds comes back, each change's inverse with it, so `undo`
    // works on a log read from disk.
    let Record::Entry(first) = &read[0] else {
        panic!("not an entry")
    };
    assert_eq!(first.request, "switch to round robin");
    assert_eq!(
        first.changes[0].what(),
        "policy round-robin (was interactive)"
    );
    assert_eq!(
        first.changes[0].inverse,
        sample().get(17).unwrap().changes[0].inverse
    );
    assert_eq!(parse(&text).unwrap(), read);
}

#[test]
fn a_request_cannot_forge_a_record() {
    let mut log = ActLog::default();
    log.push(entry(
        1,
        "hi\nE 2 t=0 reg=1 pack=- form=call tier=run consent=asked state=done",
        vec![],
    ));
    let read = parse(&log.text()).unwrap();
    assert_eq!(read.len(), 1, "{read:?}");
}

#[test]
fn malformed_text_is_refused_with_its_line() {
    let good = sample().text();
    for (bad, why) in [
        ("", "a line does not end"),
        ("actlog 3\n", "not actlog 1 or 2"),
        ("actlog 1\nX 1\n", "an unknown record"),
        ("actlog 1\nR 5\nhello\n", "text before any entry"),
        (
            "actlog 1\nE 1 t=0 reg=2 pack=- form=call tier=run consent=a state=done\n",
            "another registry version",
        ),
        (
            "actlog 1\nE 1 t=0 reg=1 pack=- form=call tier=run state=done\n",
            "out of order",
        ),
        (
            "actlog 1\nE 01 t=0 reg=1 pack=- form=call tier=run consent=a state=done\n",
            "a bad entry number",
        ),
        (
            "actlog 1\nE 1 t=0 reg=1 pack=- form=call tier=run consent=a state=done\nR 99\nshort\n",
            "runs past the end",
        ),
        (
            "actlog 1\nE 1 t=0 reg=1 pack=- form=call tier=run consent=a state=done\nD 1.2 a owner=k res=p before=a after=b inv=none\n",
            "out of order",
        ),
        (
            "actlog 1
E 1 t=0 reg=1 pack=12ab form=call tier=run consent=a state=done
",
            "a bad pack_id",
        ),
    ] {
        let error = parse(bad).unwrap_err();
        assert!(error.why.contains(why), "{bad:?}: {error}");
    }
    assert!(parse(&good[..good.len() - 1]).is_err());
}

#[test]
fn live_entries_and_overlaps_decide_what_undo_may_take() {
    let mut log = ActLog::default();
    log.push(entry(
        1,
        "switch",
        vec![change("round-robin", "interactive")],
    ));
    log.push(entry(2, "how many cpus", vec![]));
    log.push(entry(
        3,
        "switch back",
        vec![change("interactive", "round-robin")],
    ));
    assert_eq!(log.newest_live(), Some(3));
    assert_eq!(log.later_touching(1), [3]);
    assert!(log.later_touching(3).is_empty());
    log.get_mut(3).unwrap().state = State::Undone;
    assert_eq!(log.newest_live(), Some(1));
    assert!(log.later_touching(1).is_empty());
    assert_eq!(
        log.list(),
        "#1 switch: ran\n#2 how many cpus: ran\n#3 switch back: ran"
    );
}

/// bug-hunt-3 H3-24: what the log writes reads back as it was. An inverse's
/// names keep a `:` (a time zone's `+05:30`) or a space; a word with parts
/// left over (a `:` written before names were escaped) is not read as other
/// values; and a resource read back from disk, `_` for its spaces, still
/// overlaps the same one in memory.
#[test]
fn what_the_log_writes_reads_back_as_it_was() {
    use crate::undo::{Name, inverse_word, parse_inverse};
    let zone = Inverse::Appearance {
        setting: Name::new("time_zone").unwrap(),
        before: Name::new("+05:30").unwrap(),
        after: Name::new("UTC").unwrap(),
    };
    let spaced = Inverse::Keep {
        app: Name::new("my app").unwrap(),
        on: true,
    };
    for inverse in [zone, spaced] {
        let word = inverse_word(Some(inverse));
        assert!(!word.contains(' '), "{word}");
        assert_eq!(parse_inverse(&word), Some(Some(inverse)), "{word}");
    }
    assert_eq!(parse_inverse("appearance:time_zone:+05:30:UTC"), None);
    let grant = "weather's network to api.weather.example";
    let mut log = ActLog::default();
    let mut first = entry(1, "allow weather", vec![change("always", "asks")]);
    first.changes[0].res = grant.into();
    first.changes[0].inverse = Some(zone);
    log.push(first);
    let mut log = ActLog::from_records(parse(&log.text()).unwrap());
    assert_eq!(log.get(1).unwrap().changes[0].inverse, Some(zone));
    let mut second = entry(2, "revoke it", vec![change("asks", "always")]);
    second.changes[0].res = grant.into();
    log.push(second);
    assert_eq!(log.later_touching(1), [2]);
}

/// A slot rollback: an older shell reads a log a newer one wrote. An inverse
/// it cannot read leaves that change without an undo; the log, and every
/// other entry's undo, still read, and the log it writes back keeps the
/// inverse word for word for the newer shell.
#[test]
fn an_inverse_this_shell_cannot_read_leaves_only_that_change_stuck() {
    let text = "actlog 1\nE 1 t=0 reg=1 pack=- form=call tier=run consent=a state=done\nD 1.1 a owner=k res=p before=a after=b inv=maybe:7\nE 2 t=0 reg=1 pack=- form=call tier=run consent=a state=done\nD 2.1 a owner=k res=q before=a after=b inv=fs:3\n";
    let log = ActLog::from_records(parse(text).unwrap());
    let unread = log.get(1).unwrap().changes[0].inverse;
    assert!(unread.is_some_and(|i| !i.is_read()), "{unread:?}");
    assert!(!log.get(1).unwrap().live());
    assert_eq!(log.newest_live(), Some(2));
    assert!(log.text().contains(" inv=maybe:7\n"), "{}", log.text());
    // One too long, or not one word, is lost as before.
    assert_eq!(
        crate::undo::Raw::new(&"x".repeat(crate::undo::MAX_RAW + 1)),
        None
    );
    assert_eq!(crate::undo::Raw::new("a\tb"), None);
}

#[test]
fn the_ring_keeps_the_newest_64() {
    let mut log = ActLog::default();
    for id in 1..=70 {
        log.push(entry(id, &id.to_string(), vec![]));
    }
    assert!(log.get(6).is_none());
    assert!(log.get(7).is_some());
    assert_eq!(parse(&log.text()).unwrap().len(), KEEP);
}

/// An entry agent `name` made.
fn agents(id: u64, name: &str) -> Entry {
    Entry {
        agent: agentid::Name::new(name).unwrap(),
        ..entry(id, "tidy the downloads folder", vec![change("a", "b")])
    }
}

#[test]
fn a_log_of_the_persons_own_entries_stays_actlog_1() {
    let log = sample();
    let text = log.text();
    assert!(text.starts_with("actlog 1\n"), "{text}");
    assert!(!text.contains("agent="), "{text}");
    for record in text::parse(&text).unwrap() {
        if let Record::Entry(e) = record {
            assert!(e.agent.is_kai());
        }
    }
}

#[test]
fn an_agents_entry_makes_actlog_2_with_every_entry_named() {
    let mut log = sample();
    log.push(agents(30, "goals"));
    let text = log.text();
    assert!(text.starts_with("actlog 2\n"), "{text}");
    let named = text.lines().filter(|l| l.starts_with("E ")).count();
    let stamped = text
        .lines()
        .filter(|l| {
            l.starts_with("E ") && (l.ends_with(" agent=kai") || l.ends_with(" agent=goals"))
        })
        .count();
    assert_eq!(named, stamped, "{text}");
    let back = text::parse(&text).unwrap();
    let names: Vec<String> = back
        .iter()
        .filter_map(|r| match r {
            Record::Entry(e) if e.of.is_none() => Some(e.agent.as_str().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(names.last().map(String::as_str), Some("goals"));
    assert!(
        names[..names.len() - 1].iter().all(|n| n == "kai"),
        "{names:?}"
    );
}

#[test]
fn an_actlog_2_entry_with_a_bad_or_missing_agent_is_set_aside_alone() {
    let mut log = sample();
    log.push(agents(30, "goals"));
    let text = log.text();
    let whole = text::parse(&text).unwrap().len();
    for broken in [
        text.replace(" agent=goals", " agent=Goals!"),
        text.replace(" agent=goals", ""),
    ] {
        let (records, aside) = text::parse_setting_aside(&broken).unwrap();
        assert_eq!(aside, [30], "{broken}");
        assert_eq!(records.len(), whole - 1, "only that entry goes");
        assert!(
            !records
                .iter()
                .any(|r| matches!(r, Record::Entry(e) if e.id == 30))
        );
    }
    // An `agent=` field in a version 1 log is one field too many.
    let v1 = text.replacen("actlog 2", "actlog 1", 1);
    assert!(text::parse(&v1).is_err());
}

#[test]
fn the_list_names_an_agents_entry() {
    let mut log = sample();
    log.push(agents(30, "goals"));
    let list = log.list();
    assert!(
        list.contains("#30 [goals] tidy the downloads folder"),
        "{list}"
    );
    assert!(!list.contains("[kai]"), "{list}");
}
