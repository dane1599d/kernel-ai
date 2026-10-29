use super::*;
use alloc::vec;

pub(crate) const SAMPLE: &str = "kai-grants 1\ngrant 41 app=notes kind=dir object=/home/user/notes rights=read+write+enumerate from=picker life=persistent signer=- made=2026-11-02T10:14:00Z\nrule 57 action=proc_stop target=build-3 from=prompt:212 made=2026-11-02T10:20:00Z\n";

fn refused(text: &str) -> Error {
    parse(text).expect_err(text)
}

#[test]
fn adr_0001s_example_reads_and_writes_back_the_same() {
    let grants = parse(SAMPLE).unwrap();
    assert_eq!(
        grants.grants,
        [Grant {
            id: 41,
            app: "notes".into(),
            kind: Kind::Dir,
            object: "/home/user/notes".into(),
            rights: vec!["read".into(), "write".into(), "enumerate".into()],
            from: From::Picker,
            life: Life::Persistent,
            signer: None,
            made: Time::parse("2026-11-02T10:14:00Z").unwrap(),
        }]
    );
    assert_eq!(
        grants.rules,
        [Rule {
            id: 57,
            action: "proc_stop".into(),
            target: Some("build-3".into()),
            prompt: 212,
            made: Time::parse("2026-11-02T10:20:00Z").unwrap(),
        }]
    );
    assert_eq!(grants.text().unwrap(), SAMPLE);
}

#[test]
fn an_empty_file_is_its_header() {
    assert_eq!(parse("kai-grants 1\n").unwrap(), Grants::default());
    assert_eq!(Grants::default().text().unwrap(), "kai-grants 1\n");
}

#[test]
fn any_line_out_of_shape_refuses_the_whole_file() {
    let rule = "rule 1 action=service_stop target=- from=prompt:3 made=2026-11-02T10:20:00Z";
    for (bad, line) in [
        ("kai-grants 2\n", 1),
        ("kai-grants 1", 1),
        (&*format!("kai-grants 1\n{rule}"), 2),
        // Reordered, unknown, missing and extra keys.
        (
            "kai-grants 1\nrule 1 target=- action=x from=prompt:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1 action=x target=- from=prompt:3 made=2026-11-02T10:20:00Z why=x\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1 action=x target=- made=2026-11-02T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1 action=x tgt=- from=prompt:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        // Values out of shape.
        (
            "kai-grants 1\nrule 01 action=x target=- from=prompt:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 4294967296 action=x target=- from=prompt:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1 action=X target=- from=prompt:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1 action=x target=- from=menu:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1 action=x target=- from=prompt:3 made=2026-02-30T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1 action=x target= from=prompt:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1  action=x target=- from=prompt:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        (
            "kai-grants 1\nrule 1 action=x target=a=b from=prompt:3 made=2026-11-02T10:20:00Z\n",
            2,
        ),
        ("kai-grants 1\nsession 1 action=x\n", 2),
        ("kai-grants 1\n\n", 2),
        // An id twice, across kinds too.
        (&*format!("kai-grants 1\n{rule}\n{rule}\n"), 3),
    ] {
        assert_eq!(refused(bad).line, line, "{bad:?}");
    }
    for object in [
        "notes",
        "/home/user/../system",
        "/home//user",
        "/home/user/",
    ] {
        let text = SAMPLE.replace("/home/user/notes", object);
        assert_eq!(refused(&text).line, 2, "{object}");
    }
}

#[test]
fn a_signer_is_64_lowercase_hex_digits() {
    let key = "9f".repeat(32);
    let text = SAMPLE.replace("signer=-", &format!("signer={key}"));
    let grants = parse(&text).unwrap();
    assert_eq!(grants.grants[0].signer, Some([0x9f; 32]));
    assert_eq!(grants.text().unwrap(), text);
    for bad in ["9F".repeat(32), "9f".repeat(31), "zz".repeat(32)] {
        let text = SAMPLE.replace("signer=-", &format!("signer={bad}"));
        assert!(parse(&text).is_err(), "{bad}");
    }
}

#[test]
fn session_grants_are_dropped_at_boot_and_rules_kept() {
    let text = SAMPLE.replace("life=persistent", "life=session");
    let booted = parse(&text).unwrap().at_boot();
    assert!(booted.grants.is_empty());
    assert_eq!(booted.rules.len(), 1);
}

#[test]
fn a_record_that_would_not_read_back_is_not_written() {
    let mut grants = parse(SAMPLE).unwrap();
    grants.rules[0].target = Some(String::from("two words"));
    assert!(grants.text().is_err());
    grants.rules[0].target = Some("x".repeat(MAX_LINE));
    assert!(grants.text().is_err());
}

#[test]
fn times_read_and_write_in_one_shape() {
    for text in [
        "1970-01-01T00:00:00Z",
        "2024-02-29T23:59:59Z",
        "9999-12-31T23:59:59Z",
    ] {
        assert_eq!(Time::parse(text).unwrap().format(), text);
    }
    assert_eq!(
        Time::from_ns(1_792_618_800_123_456_789).format(),
        "2026-10-21T21:40:00Z"
    );
    assert_eq!(Time(u64::MAX).format(), "9999-12-31T23:59:59Z");
    for bad in [
        "2023-02-29T00:00:00Z",
        "2026-11-02 10:20:00Z",
        "1969-12-31T23:59:59Z",
    ] {
        assert!(Time::parse(bad).is_err(), "{bad}");
    }
}
