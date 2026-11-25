//! A hostile `kai-memory 1` file (ADR 0013 section 3), as one read back from the
//! `memory` volume, where anyone with the person's files can edit it: cut, mutated or
//! bent at every byte, it is read or refused, never a panic. A bad header refuses the
//! whole file; a bad record is skipped and reported, and the rest is read. What is
//! read allocates no more than its bytes could hold, never keeps an id the next one
//! has not passed, an id twice, a text twice in a scope or more than the store holds,
//! and is written back as a file that reads back as the same memories, with nothing
//! skipped.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kaimem::record::{Decoded, HEADER, MAX_MEMORIES, MAX_TEXT, Memory, Scope, Why, decode, encode};

fn sample() -> String {
    let memories = [
        Memory {
            id: 1,
            time: 1_792_618_800_123_456_789,
            source: 17,
            scope: Scope::All,
            text: String::from("I use metric units"),
        },
        Memory {
            id: 4,
            time: 1_792_618_900_000_000_000,
            source: 21,
            scope: Scope::Agent(String::from("goals")),
            text: String::from("the report lives in /home/user/notes \u{2014} caf\u{e9}"),
        },
        Memory {
            id: 6,
            time: 5,
            source: 0,
            scope: Scope::Agent(String::from("maint")),
            text: "x".repeat(MAX_TEXT),
        },
    ];
    encode(9, &memories)
}

fn read(bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let (read, peak) = hostile::peak_alloc(|| decode(&text));
    assert!(
        peak <= 16 * 1024 + 16 * bytes.len(),
        "{peak} bytes for {} bytes",
        bytes.len()
    );
    let Ok(Decoded {
        next,
        memories,
        skipped,
    }) = read
    else {
        assert!(!text.starts_with(&format!("{HEADER} next=")) || !header_ok(&text));
        return;
    };
    // What was read keeps the file's and the store's rules: ids below the next one,
    // none twice, no text twice in a scope, at most the store's limit, every text one
    // line of at most the limit.
    assert!(memories.len() <= MAX_MEMORIES);
    for (i, memory) in memories.iter().enumerate() {
        assert!(memory.id < next, "{text:?}");
        assert!(!memories[..i].iter().any(|m| m.id == memory.id), "{text:?}");
        assert!(
            !memories[..i]
                .iter()
                .any(|m| m.scope == memory.scope && m.text.eq_ignore_ascii_case(&memory.text)),
            "{text:?}"
        );
        assert!(memory.text.len() <= MAX_TEXT && !memory.text.contains('\n'));
    }
    // Every line after the header is in a record read or in one skipped: each record
    // read is two lines, and the reports name lines in order, never overlapping.
    let lines = text.lines().count();
    let reported: usize = skipped.iter().map(|s| s.last + 1 - s.first).sum();
    assert_eq!(2 * memories.len() + reported, lines - 1, "{text:?}");
    for pair in skipped.windows(2) {
        assert!(pair[0].last < pair[1].first, "{skipped:?}");
    }
    // It is written as a file that reads back as the same memories with nothing
    // skipped, and the written file is the one form: it writes itself.
    let written = encode(next, &memories);
    let again = decode(&written).unwrap();
    assert_eq!((again.next, &again.memories), (next, &memories), "{text:?}");
    assert!(again.skipped.is_empty(), "{text:?}");
    assert_eq!(encode(next, &again.memories), written, "{text:?}");
}

/// Whether `text`'s first line is a `kai-memory 1` header.
fn header_ok(text: &str) -> bool {
    text.lines()
        .next()
        .and_then(|line| line.strip_prefix(HEADER)?.strip_prefix(" next="))
        .is_some_and(|next| next.parse::<u64>().is_ok())
}

#[test]
fn the_sample_reads_and_is_its_own_canonical_form() {
    let sample = sample();
    let read = decode(&sample).unwrap();
    assert_eq!(
        (read.next, read.memories.len(), read.skipped.len()),
        (9, 3, 0)
    );
    assert_eq!(encode(read.next, &read.memories), sample);
}

#[test]
fn mutated_files_are_read_or_refused_and_what_is_read_reads_back() {
    hostile::fuzz(sample().as_bytes(), 53, 60_000, read);
}

#[test]
fn every_prefix_of_a_file_is_read_or_refused() {
    hostile::truncations(sample().as_bytes(), |_, cut| read(cut));
}

#[test]
fn a_file_with_a_bad_header_is_refused_whole() {
    for bad in [
        String::new(),
        HEADER.to_string(),
        format!("{HEADER} next=\n"),
        format!("{HEADER} next=-1\n"),
        format!("{HEADER} next=18446744073709551616\n"),
        "kai-memory 2 next=9\n".to_string(),
        "kai-memory 1  next=9\n".to_string(),
    ] {
        assert!(decode(&bad).is_err(), "{bad:?}");
    }
}

#[test]
fn a_record_that_cannot_be_the_stores_is_skipped_and_its_neighbours_are_read() {
    let good = "id=2 time=5 source=10 scope=all\nI use metric units\n";
    for (bad, why) in [
        // An id the next one has not passed, and one twice.
        (
            "id=9 time=5 source=10 scope=all\nI like tea\n".to_string(),
            Why::IdNotBelowNext(9),
        ),
        (
            "id=2 time=6 source=11 scope=all\nsomething else\n".to_string(),
            Why::IdTwice(2),
        ),
        // A record cut after its fields.
        ("id=1 time=5 source=10 scope=all\n".to_string(), Why::NoText),
        // Text on two lines, and text that is empty.
        (
            "id=1 time=5 source=10 scope=all\nfirst\nsecond\n".to_string(),
            Why::ExtraLines,
        ),
        (
            "id=1 time=5 source=10 scope=all\n\n".to_string(),
            Why::BadText,
        ),
        // A scope that is not an agent's name.
        (
            "id=1 time=5 source=10 scope=Goals\nx\n".to_string(),
            Why::BadFields,
        ),
        (
            "id=1 time=5 source=10 scope=a\nx\n".to_string(),
            Why::BadFields,
        ),
        // Text over the limit, and text with a control or a bidi character.
        (
            format!(
                "id=1 time=5 source=10 scope=all\n{}\n",
                "x".repeat(MAX_TEXT + 1)
            ),
            Why::BadText,
        ),
        (
            "id=1 time=5 source=10 scope=all\na\u{7}b\n".to_string(),
            Why::BadText,
        ),
        (
            "id=1 time=5 source=10 scope=all\na\u{202e}b\n".to_string(),
            Why::BadText,
        ),
        (
            "id=1 time=5 source=10 scope=all\na\u{200b}b\n".to_string(),
            Why::BadText,
        ),
        // The same text in the same scope, by case.
        (
            "id=1 time=5 source=10 scope=all\ni use METRIC units\n".to_string(),
            Why::SameText(1),
        ),
    ] {
        let file = format!("{HEADER} next=9\n{good}{bad}");
        let read = decode(&file).unwrap_or_else(|e| panic!("{e}: {file:?}"));
        assert_eq!(read.memories.len(), 1, "{file:?}");
        assert_eq!(read.memories[0].text, "I use metric units", "{file:?}");
        assert_eq!(read.skipped.len(), 1, "{file:?}");
        assert_eq!(read.skipped[0].why, why, "{file:?}");
        assert_eq!(read.skipped[0].first, 4, "{file:?}");
    }
}
