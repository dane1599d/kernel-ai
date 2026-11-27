//! The bodies of `supervise`'s state fuzz targets (plan P6 G26), which
//! `supervise/tests/hostile.rs` also runs over hostile mutations
//! (`testkit/hostile.rs`), the way `fuzz/harness.rs` serves its targets: a
//! later `fuzz/fuzz_targets/supervise_state.rs` includes this file by `#[path]`
//! and calls one of these with libFuzzer's input. Each must return for any
//! input, and only a bug in `supervise` may panic.
//!
//! What each holds: the supervisor's saved state, a `keep` request, a
//! `profile` request and its `ok` answer are read so that damaged bytes give
//! nothing (or an error), never a panic, and never more than their text says:
//! whatever is taken is exactly what its own encoding writes back.

#![allow(dead_code)]

use supervise::profile::{self, Set};
use supervise::{Record, decode, encode, keep};

/// The pid a `profile` answer is read for.
pub const ANSWER_PID: u64 = 31;

/// The saved state a supervisor reads at start (`supervise::decode`). A
/// damaged region gives fewer records, never a panic: each record comes from
/// one line after the magic, has a name with no space in it, and the state
/// they re-encode to decodes to exactly them, so no field holds more than the
/// encoding writes.
pub fn supervisor_state(data: &[u8]) {
    let records = decode(data);
    if records.is_empty() {
        return;
    }
    let text = core::str::from_utf8(data).expect("records only from UTF-8");
    assert!(records.len() < text.lines().count(), "{text:?}");
    for record in &records {
        assert!(!record.name.is_empty(), "{text:?}");
        assert!(!record.name.contains(char::is_whitespace), "{text:?}");
    }
    let again = encode(&records);
    assert_eq!(decode(again.as_bytes()), records, "{text:?}");
}

/// `keep <name> on|off`: taken only as its three words, with a valid name, and
/// its encoding reads back as the same switch.
pub fn keep_request(data: &[u8]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Some((name, on)) = keep::parse(text) else {
        return;
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(
        words,
        ["keep", name, if on { "on" } else { "off" }],
        "{text:?}"
    );
    assert!(name.len() <= kabi::MAX_NAME, "{text:?}");
    assert!(
        name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "{text:?}"
    );
    assert_eq!(keep::parse(&keep::encode(name, on)), Some((name, on)));
}

/// `profile <pid> <class> <pages|-> [cap]`: a request taken is one of the two
/// classes, and is exactly its own encoding, so no other spelling of a pid or
/// a quota reaches the kernel.
pub fn profile_request(data: &[u8]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Some(set) = Set::parse(text) else {
        return;
    };
    assert!(set.class <= 1, "{text:?}");
    assert_eq!(set.encode().as_deref(), Some(text), "{text:?}");
}

/// The supervisor's `ok <class> <pages|->` answer, read back for an undo: for
/// the pid asked about, never `cap`, one of the two classes, and exactly its
/// own encoding. An `err` or a damaged answer is an error, never a panic.
pub fn profile_answer(data: &[u8]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Ok(before) = profile::parse_before(ANSWER_PID, text) else {
        return;
    };
    assert_eq!(before.pid, ANSWER_PID, "{text:?}");
    assert!(!before.cap && before.class <= 1, "{text:?}");
    assert_eq!(profile::encode_before(before), text, "{text:?}");
}

/// Samples for the hostile runs: saved states of every version still read.
pub fn state_samples() -> Vec<Vec<u8>> {
    let mut shell = Record::new("shell");
    shell.restarts = 3;
    shell.quick_failures = 1;
    shell.recent = vec![1_000, 2_500, 90_000];
    shell.pid = 12;
    shell.started_ms = 90_001;
    let mut notes = Record::new("notes");
    notes.stopped = true;
    notes.keep = true;
    let mut net = Record::new("net-0");
    net.gave_up = true;
    vec![
        encode(&[shell, notes, net]).into_bytes(),
        b"supervisor-state 4\nshell 3 1 0 1 12 90001 1000 2500\nfs 0 0 0 0 0 0".to_vec(),
        b"supervisor-state 3\nshell 3 1 1 12 90001 1000\nfs 0 0 0 0 0".to_vec(),
    ]
}
