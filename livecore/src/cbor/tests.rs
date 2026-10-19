use super::*;
use crate::state;
use alloc::vec;

/// `notes` 2 after `add buy milk` and `add walk`, as minicbor 2.3.0 encodes it.
const NOTES_V2: &[u8] = &[
    0x82, 0x02, 0x82, 0x82, 0x83, 0x01, 0x68, b'b', b'u', b'y', b' ', b'm', b'i', b'l', b'k', 0x02,
    0x83, 0x02, 0x64, b'w', b'a', b'l', b'k', 0x01, 0x03,
];
/// `notes` 1 holding the same two texts.
const NOTES_V1: &[u8] = &[
    0x82, 0x01, 0x81, 0x82, 0x68, b'b', b'u', b'y', b' ', b'm', b'i', b'l', b'k', 0x64, b'w', b'a',
    b'l', b'k',
];

fn shown(bytes: &[u8], width: usize) -> Vec<String> {
    render(&parse(bytes).unwrap(), width, 50)
}

fn one_line(bytes: &[u8]) -> String {
    let lines = shown(bytes, 200);
    assert_eq!(lines.len(), 1, "{lines:?}");
    lines[0].clone()
}

#[test]
fn notes_states_read_as_their_schema_and_body() {
    assert_eq!(
        state::view(NOTES_V2, 80, 20).unwrap(),
        (
            2,
            vec![String::from(r#"[[[1, "buy milk", 2], [2, "walk", 1]], 3]"#)]
        )
    );
    assert_eq!(
        state::view(NOTES_V1, 80, 20).unwrap(),
        (1, vec![String::from(r#"[["buy milk", "walk"]]"#)])
    );
    let empty = [0x82, 0x02, 0x82, 0x80, 0x01];
    assert_eq!(
        state::view(&empty, 80, 20).unwrap(),
        (2, vec![String::from("[[], 1]")])
    );
    for not_a_state in [&[0x01][..], &[0x82, 0x61, b'x', 0x01], &[0x81, 0x02]] {
        assert!(state::view(not_a_state, 80, 20).is_err(), "{not_a_state:?}");
    }
}

#[test]
fn what_does_not_fit_breaks_one_element_per_line() {
    let (_, lines) = state::view(NOTES_V2, 30, 20).unwrap();
    assert_eq!(
        lines,
        [
            "[",
            "  [",
            r#"    [1, "buy milk", 2],"#,
            r#"    [2, "walk", 1]"#,
            "  ],",
            "  3",
            "]",
        ]
    );
}

#[test]
fn every_kind_of_item_is_shown() {
    // {1: -1, "a": h'00ff', 2: true, 3: f16(0x3c00), 4: 1("x")}
    let map = [
        0xa5, 0x01, 0x20, 0x61, b'a', 0x42, 0x00, 0xff, 0x02, 0xf5, 0x03, 0xf9, 0x3c, 0x00, 0x04,
        0xc1, 0x61, b'x',
    ];
    assert_eq!(
        one_line(&map),
        r#"{1: -1, "a": h'00ff', 2: true, 3: f16(0x3c00), 4: 1("x")}"#
    );
    assert_eq!(
        one_line(&[0x84, 0xf4, 0xf6, 0xf7, 0xf0]),
        "[false, null, undefined, simple(16)]"
    );
    assert_eq!(one_line(&[0xf8, 0x20]), "simple(32)");
    assert_eq!(one_line(&[0xfa, 0x3f, 0x80, 0, 0]), "f32(0x3f800000)");
    assert_eq!(
        one_line(&[0xfb, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0]),
        "f64(0x3ff0000000000000)"
    );
    assert_eq!(
        one_line(&[0x3b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
        "-18446744073709551616"
    );
    assert_eq!(
        one_line(&[0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
        "18446744073709551615"
    );
}

#[test]
fn indefinite_lengths_end_at_their_break() {
    assert_eq!(one_line(&[0x9f, 0x01, 0x02, 0xff]), "[1, 2]");
    assert_eq!(one_line(&[0xbf, 0x61, b'a', 0x01, 0xff]), r#"{"a": 1}"#);
    assert_eq!(one_line(&[0x7f, 0x61, b'a', 0x61, b'b', 0xff]), r#""ab""#);
    assert_eq!(one_line(&[0x5f, 0x41, 0x00, 0x41, 0x01, 0xff]), "h'0001'");
    for bad in [
        &[0x7f, 0x41, 0x00, 0xff][..],
        &[0x7f, 0x7f, 0xff, 0xff],
        &[0x9f, 0x01],
        &[0xbf, 0x01, 0x02],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn text_is_escaped_to_printable_ascii() {
    // "\"\\\n" then U+00E9 and a byte that is not UTF-8.
    let text = [0x66, b'"', b'\\', b'\n', 0xc3, 0xa9, 0xff];
    assert_eq!(one_line(&text), r#""\"\\\n\u{e9}\xff""#);
}

#[test]
fn malformed_items_are_refused() {
    let huge_bytes = [0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let huge_array = [0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    let huge_map = [0xba, 0x00, 0x01, 0x00, 0x00, 0x01, 0x02];
    for bad in [
        &[][..],
        &[0x82, 0x01],
        &huge_bytes,
        &huge_array,
        &huge_map,
        &[0x62, b'a'],
        &[0xff],
        &[0x1c],
        &[0x1f],
        &[0xdf, 0x01],
        &[0xf8, 0x1f],
        &[0xf9, 0x3c],
        &[0x01, 0x02],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn nesting_is_capped() {
    let nested = |depth: usize| {
        let mut bytes = vec![0x81; depth];
        bytes.push(0x01);
        bytes
    };
    assert!(parse(&nested(MAX_DEPTH)).is_ok());
    assert!(parse(&nested(MAX_DEPTH + 1)).is_err());
    let mut tags = vec![0xc1; MAX_DEPTH + 1];
    tags.push(0x01);
    assert!(parse(&tags).is_err());
}

#[test]
fn reading_stops_at_the_item_cap() {
    let count = MAX_ITEMS + 100;
    let mut bytes = vec![0x99, (count >> 8) as u8, count as u8];
    bytes.resize(bytes.len() + count, 0x00);
    let Item::Array(items) = parse(&bytes).unwrap() else {
        panic!("not an array");
    };
    assert_eq!(items.len(), MAX_ITEMS);
    assert_eq!(items.last(), Some(&Item::More));
}

#[test]
fn long_items_are_cut_to_the_width() {
    let mut text = vec![0x78, 100];
    text.resize(102, b'a');
    let lines = shown(&text, 20);
    assert_eq!(lines, [format!("\"{}...", "a".repeat(16))]);
    let mut bytes = vec![0x58, 100];
    bytes.resize(102, 0xab);
    let lines = shown(&bytes, 20);
    assert_eq!(lines, [format!("h'{}...", &"ab".repeat(8)[..15])]);
    // Text that is not UTF-8 is cut the same way, and escaping it stops soon
    // after the limit rather than escaping every byte.
    let mut invalid = vec![0x79, 0xea, 0x60];
    invalid.resize(invalid.len() + 60_000, 0xff);
    let lines = shown(&invalid, 20);
    assert_eq!(lines, [format!("\"{}...", "\\xff".repeat(4))]);
    let mut out = String::new();
    leaf_text(&parse(&invalid).unwrap(), &mut out, 20);
    assert!(out.len() <= 25, "{}", out.len());
}

#[test]
fn maps_and_tags_that_do_not_fit_break_too() {
    // {"a": [1, 2, 3, 4, 5], "b": 2}
    let map = [0xa2, 0x61, b'a', 0x85, 1, 2, 3, 4, 5, 0x61, b'b', 0x02];
    assert_eq!(
        shown(&map, 14),
        [
            "{",
            r#"  "a": ["#,
            "    1,",
            "    2,",
            "    3,",
            "    4,",
            "    5",
            "  ],",
            r#"  "b": 2"#,
            "}",
        ]
    );
    // A key longer than half the width is cut.
    let mut long_key = vec![0xa1, 0x78, 40];
    long_key.resize(long_key.len() + 40, b'a');
    long_key.push(0x01);
    assert_eq!(shown(&long_key, 20), ["{", r#"  "aaaaaa...: 1"#, "}"]);
    // 1([1, 2, 3, 4, 5])
    let tag = [0xc1, 0x85, 1, 2, 3, 4, 5];
    assert_eq!(
        shown(&tag, 10),
        ["1([", "  1,", "  2,", "  3,", "  4,", "  5", "])"]
    );
}

#[test]
fn lines_past_the_rows_are_left_out() {
    let ten: Vec<u8> = [0x8a].into_iter().chain(0..10).collect();
    let item = parse(&ten).unwrap();
    assert_eq!(render(&item, 8, 5), ["[", "  0,", "  1,", "  2,", "..."]);
    assert_eq!(render(&item, 8, 12).len(), 12);
    assert_eq!(render(&item, 8, 12).last().unwrap(), "]");
}

#[test]
fn any_bytes_give_bounded_printable_lines_or_an_error() {
    let samples = [
        NOTES_V2.to_vec(),
        NOTES_V1.to_vec(),
        vec![0xbf, 0x61, b'a', 0x9f, 0x01, 0xff, 0xff],
    ];
    let mut seed: u32 = 7;
    let mut next = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        seed
    };
    for sample in &samples {
        for _ in 0..2000 {
            let mut bytes = sample.clone();
            let r = next();
            let at = (r as usize >> 8) % bytes.len();
            bytes[at] = (next() >> 16) as u8;
            if r % 3 == 0 {
                bytes.truncate(at + 1);
            }
            if let Ok(item) = parse(&bytes) {
                for (width, rows) in [(8, 3), (24, 12), (80, 40)] {
                    let lines = render(&item, width, rows);
                    assert!(lines.len() <= rows, "{bytes:02x?}");
                    for line in &lines {
                        assert!(line.len() <= width, "{line:?} from {bytes:02x?}");
                        assert!(line.bytes().all(|b| (b' '..=b'~').contains(&b)), "{line:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_state_command_shows_a_header_then_the_state_with_a_lead() {
    let mut ledger = crate::ledger::Ledger::new();
    assert_eq!(
        state::lines("notes", &ledger, 80, 10),
        ["notes has no saved state"]
    );
    ledger.checkpoint = NOTES_V2.to_vec();
    let header = "notes state, saved by none: schema 2, 25 bytes";
    assert_eq!(
        state::lines("notes", &ledger, 80, 10),
        [header, r#"= [[[1, "buy milk", 2], [2, "walk", 1]], 3]"#]
    );
    assert_eq!(
        state::lines("notes", &ledger, 30, 4),
        [header, "= [", "=   [", "= ..."]
    );
    ledger.checkpoint = vec![0x82, 0x02];
    assert!(state::lines("notes", &ledger, 80, 10)[0].contains("does not read"));
}

#[test]
fn a_shown_state_is_recognised_by_its_schema() {
    let mut ledger = crate::ledger::Ledger::new();
    ledger.checkpoint = NOTES_V2.to_vec();
    let shown = state::lines("notes", &ledger, 80, 10);
    assert!(state::shows_schema(&shown, 2));
    assert!(!state::shows_schema(&shown, 1));
    assert!(!state::shows_schema(&shown[..1], 2));
    ledger.checkpoint.clear();
    assert!(!state::shows_schema(
        &state::lines("notes", &ledger, 80, 10),
        2
    ));
}
