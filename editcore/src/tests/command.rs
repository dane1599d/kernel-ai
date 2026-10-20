use crate::command::{Command, parse};

#[test]
fn every_command_form_parses() {
    let cases = [
        ("files", Command::Files),
        ("open lib.rs", Command::Open("lib.rs")),
        (
            "p",
            Command::Print {
                from: None,
                to: None,
            },
        ),
        (
            "p 40",
            Command::Print {
                from: Some(40),
                to: None,
            },
        ),
        (
            "p 40,45",
            Command::Print {
                from: Some(40),
                to: Some(45),
            },
        ),
        ("find schema {SCHEMA}", Command::Find("schema {SCHEMA}")),
        (
            "s |a|b|",
            Command::Substitute {
                line: None,
                old: "a",
                new: "b",
            },
        ),
        (
            "s 12 #a b#c|d#",
            Command::Substitute {
                line: Some(12),
                old: "a b",
                new: "c|d",
            },
        ),
        (
            "s |x||",
            Command::Substitute {
                line: None,
                old: "x",
                new: "",
            },
        ),
        ("c 3     let x = 1;", Command::Change(3, "    let x = 1;")),
        ("i 1 // top", Command::Insert(1, "// top")),
        ("a 9 ", Command::Append(9, "")),
        ("d 4", Command::Delete(4, 4)),
        ("d 4,6", Command::Delete(4, 6)),
        ("undo", Command::Undo),
        ("diff", Command::Diff),
        ("build", Command::Build),
        ("try", Command::Try),
        ("status", Command::Status),
        ("reset", Command::Reset),
        ("help", Command::Help),
    ];
    for (line, command) in cases {
        assert_eq!(parse(line), Ok(command), "{line}");
    }
}

#[test]
fn bad_lines_say_what_is_wrong() {
    for line in [
        "",
        "x",
        "open",
        "p x",
        "p 0",
        "s",
        "s |a|",
        "s |a|b|c|",
        "s aab",
        "s 3",
        "c",
        "c x y",
        "d",
        "d 2,",
        "build now",
        "find",
    ] {
        assert!(parse(line).is_err(), "{line:?}");
    }
    assert!(parse("frobnicate").unwrap_err().contains("type help"));
}
