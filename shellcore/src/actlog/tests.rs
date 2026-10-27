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
        "D 17.1 set_policy owner=kernel res=policy before=interactive after=round-robin inv=restore\n"
    ));
    assert!(text.contains("U 18 of=17 t=1792618800123456789 state=done\n"));
    assert!(text.contains(
        "S 19 t=1792618860000000000 owner=kernel 33\nnvme0 reset after 5 s timeout, ok\n"
    ));
    let read = parse(&text).unwrap();
    assert_eq!(read.len(), 3);
    // What the text holds comes back; the runtime inverse is not text.
    let Record::Entry(first) = &read[0] else {
        panic!("not an entry")
    };
    assert_eq!(first.request, "switch to round robin");
    assert_eq!(
        first.changes[0].what(),
        "policy round-robin (was interactive)"
    );
    assert_eq!(first.changes[0].inverse, None);
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
        ("actlog 2\n", "not actlog 1"),
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
            "actlog 1\nE 1 t=0 reg=1 pack=- form=call tier=run consent=a state=done\nD 1.2 a owner=k res=p before=a after=b inv=restore\n",
            "out of order",
        ),
        (
            "actlog 1\nE 1 t=0 reg=1 pack=- form=call tier=run consent=a state=done\nD 1.1 a owner=k res=p before=a after=b inv=maybe\n",
            "unknown inverse",
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
