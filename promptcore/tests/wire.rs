//! `kai-cc 1` messages read and write back, and bad ones are refused.

use promptcore::Kind;
use promptcore::wire::{Origin, Record, Reply, Request, ValueType, parse_reply};

#[test]
fn requests_read_and_write_back() {
    let entry = "I64 Sq(I64 x) {\n  return x * x;\n}";
    let eval = format!("eval 12\n{entry}");
    for text in [
        "open main origin=user",
        "open trial-3 origin=model",
        eval.as_str(),
        "replay 4\nI64 x;",
        "resume 12 ok\n{\"status\":0}",
        "resume 12 err\nrefused",
        "undo",
        "dis Sq",
        "defs",
        "reset",
        "build hello-app 0.0.12",
        "eval 0\n",
    ] {
        let request = Request::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(request.text(), text);
    }
    assert_eq!(
        Request::parse("open main origin=model").unwrap(),
        Request::Open {
            session: "main",
            origin: Origin::Model
        }
    );
    let too_long = format!("eval 1\n{}", "x".repeat(promptcore::MAX_ENTRY + 1));
    for text in [
        "",
        "eval 1",
        "eval 01\nx",
        "eval -1\nx",
        "eval 18446744073709551616\nx",
        "open Main origin=user",
        "open main origin=root",
        "open main  origin=user",
        "undo\n",
        "undo now",
        "dis 9lives",
        "dis a-b",
        "resume 1 maybe\nx",
        "build ../x 0.0.1",
        "build hello",
        "build hello 0.0.1 x",
        "build hello 1.0-rc",
        "build hello 0.0.1\nint x;",
        too_long.as_str(),
    ] {
        assert!(Request::parse(text).is_err(), "{text:?}");
    }
    let long_header = format!("dis {}", "a".repeat(200));
    assert!(Request::parse(&long_header).is_err());
}

fn all_records() -> Vec<Record<'static>> {
    vec![
        Record::Out(b"line one\nline two\n\xff"),
        Record::Def {
            kind: Kind::Func,
            slot: Some(7),
            name: "Sq",
        },
        Record::Def {
            kind: Kind::Type,
            slot: None,
            name: "struct Point",
        },
        Record::Retyped("Sq"),
        Record::Value {
            ty: ValueType::F64,
            text: b"2.5",
        },
        Record::Done {
            compile_us: 43_000,
            link_us: 120,
            run_us: 7,
        },
    ]
}

#[test]
fn replies_read_and_write_back() {
    let terminals = [
        Record::Done {
            compile_us: 1,
            link_us: 0,
            run_us: 18_446_744_073_709_551_615,
        },
        Record::Need {
            id: 3,
            call: br#"{"name":"fs_trash","arguments":{"path":"/home/user/x"}}"#,
        },
        Record::Err {
            line: 2,
            text: b"/w/entry.c(9): error: undeclared\n  x + 1\n  ^",
        },
        Record::Trap {
            reason: "unreachable",
            function: Some("Boom"),
        },
        Record::Trap {
            reason: "deadline",
            function: None,
        },
        Record::Undone(5),
        Record::Text(b"fn      I64 Sq(I64 x);\n"),
        Record::Ok,
    ];
    for terminal in terminals {
        let mut reply = Reply::new(8192);
        let mut records = all_records();
        records.pop();
        records.push(terminal);
        for record in &records {
            reply.push(record).unwrap();
        }
        let bytes = reply.into_bytes();
        assert_eq!(parse_reply(&bytes).unwrap(), records);
    }
}

#[test]
fn a_reply_stops_at_its_end_and_its_cap() {
    let mut reply = Reply::new(64);
    reply.push(&Record::Out(b"hello")).unwrap();
    assert!(reply.push(&Record::Out(&[b'x'; 60])).is_err());
    assert_eq!(reply.room(), 64 - 11);
    assert!(reply.push_leaving(&Record::Out(b"abc"), 50).is_err());
    reply.push_leaving(&Record::Out(b""), 40).unwrap();
    reply.push(&Record::Ok).unwrap();
    assert!(reply.push(&Record::Out(b"late")).is_err());
    assert_eq!(reply.into_bytes(), b"out 5\nhelloout 0\nok\n");
}

#[test]
fn bad_replies_are_refused() {
    for bytes in [
        &b""[..],
        b"out 3\nabc",
        b"ok\nok\n",
        b"ok\nout 1\nx",
        b"out 9\nabc\nok\n",
        b"out 3\nabcok",
        b"done 1 2\n",
        b"done 1 2 03\n",
        b"def fn 7 3\nS\nqok\n",
        b"def fun - 2\nSqok\n",
        b"def fn 4294967296 2\nSqok\n",
        b"trap Unreachable -\n",
        b"trap x 9f\n",
        b"value i32 1\n1ok\n",
        b"undone\n",
        b"err 1 18446744073709551615\nx",
        b"ok",
        b"\xff\n",
    ] {
        assert!(
            parse_reply(bytes).is_err(),
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
    }
    let long = [b'o'; 200];
    assert!(parse_reply(&long).is_err());
    let many: Vec<u8> = b"out 0\n"
        .repeat(promptcore::wire::MAX_RECORDS + 1)
        .into_iter()
        .chain(b"ok\n".iter().copied())
        .collect();
    assert!(parse_reply(&many).is_err());
    assert_eq!(parse_reply(b"text 0\n").unwrap(), [Record::Text(b"")]);
}

#[test]
fn the_writer_refuses_what_the_reader_would() {
    let mut reply = Reply::new(8192);
    let long = "a".repeat(129);
    for record in [
        Record::Def {
            kind: Kind::Var,
            slot: None,
            name: &long,
        },
        Record::Def {
            kind: Kind::Var,
            slot: None,
            name: "line\nbreak",
        },
        Record::Retyped("not-a-name"),
        Record::Retyped(&long[..64]),
        Record::Trap {
            reason: "Deadline",
            function: None,
        },
        Record::Trap {
            reason: "memory",
            function: Some("9f"),
        },
    ] {
        assert!(reply.push(&record).is_err(), "{record:?}");
    }
    reply.push(&Record::Ok).unwrap();
    assert_eq!(parse_reply(&reply.into_bytes()).unwrap(), [Record::Ok]);
}

#[test]
fn a_full_reply_keeps_a_record_for_its_end() {
    let mut reply = Reply::new(usize::MAX);
    let max = promptcore::wire::MAX_RECORDS;
    for _ in 0..max - 1 {
        reply.push(&Record::Out(b"x")).unwrap();
    }
    assert!(reply.push(&Record::Out(b"x")).is_err());
    reply.push(&Record::Ok).unwrap();
    let bytes = reply.into_bytes();
    assert_eq!(parse_reply(&bytes).unwrap().len(), max);
}
