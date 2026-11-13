//! A hostile `kai-memory 1` file (ADR 0013 section 3), as one read back from the
//! `memory` volume, where anyone with the person's files can edit it: cut, mutated or
//! bent at every byte, it is read or refused whole, never a panic; what is read
//! allocates no more than its bytes could hold, never keeps an id the next one has
//! not passed or an id twice, and is written back as a file that reads back as the
//! same memories.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kaimem::record::{HEADER, MAX_TEXT, Memory, Scope, decode, encode};

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
    let Ok((next, memories)) = read else {
        return;
    };
    // What was read keeps the file's own rules: ids below the next one, none twice,
    // every text one line of at most the limit.
    for (i, memory) in memories.iter().enumerate() {
        assert!(memory.id < next, "{text:?}");
        assert!(!memories[..i].iter().any(|m| m.id == memory.id), "{text:?}");
        assert!(memory.text.len() <= MAX_TEXT && !memory.text.contains('\n'));
    }
    // It is written as a file that reads back as the same memories, and the written
    // file is the one form: it writes itself.
    let written = encode(next, &memories);
    assert_eq!(decode(&written), Ok((next, memories.clone())), "{text:?}");
    assert_eq!(
        encode(next, &decode(&written).unwrap().1),
        written,
        "{text:?}"
    );
}

#[test]
fn the_sample_reads_and_is_its_own_canonical_form() {
    let sample = sample();
    let (next, memories) = decode(&sample).unwrap();
    assert_eq!((next, memories.len()), (9, 3));
    assert_eq!(encode(next, &memories), sample);
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
fn a_file_that_cannot_be_the_stores_is_refused_whole() {
    let header = |next: &str| format!("{HEADER} next={next}\n");
    let record = "id=1 time=5 source=10 scope=all\nI use metric units\n";
    for bad in [
        String::new(),
        HEADER.to_string(),
        format!("{HEADER} next=\n"),
        format!("{HEADER} next=-1\n"),
        format!("{HEADER} next=18446744073709551616\n"),
        // An id the next one has not passed, and one twice.
        format!("{}{record}", header("1")),
        format!("{}{record}{record}", header("9")),
        // A record cut after its fields.
        format!("{}id=1 time=5 source=10 scope=all\n", header("9")),
        // Text on two lines, and text that is empty.
        format!(
            "{}id=1 time=5 source=10 scope=all\nfirst\nsecond\n",
            header("9")
        ),
        format!("{}id=1 time=5 source=10 scope=all\n\n", header("9")),
        // A scope that is not an agent's name.
        format!("{}id=1 time=5 source=10 scope=Goals\nx\n", header("9")),
        format!("{}id=1 time=5 source=10 scope=a\nx\n", header("9")),
        // Text over the limit, and text with a control or a bidi character.
        format!(
            "{}id=1 time=5 source=10 scope=all\n{}\n",
            header("9"),
            "x".repeat(MAX_TEXT + 1)
        ),
        format!("{}id=1 time=5 source=10 scope=all\na\u{7}b\n", header("9")),
        format!(
            "{}id=1 time=5 source=10 scope=all\na\u{202e}b\n",
            header("9")
        ),
        format!(
            "{}id=1 time=5 source=10 scope=all\na\u{200b}b\n",
            header("9")
        ),
    ] {
        assert!(decode(&bad).is_err(), "{bad:?}");
    }
}
