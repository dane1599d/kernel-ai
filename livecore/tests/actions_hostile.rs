//! Hostile `kai-actions 1` sections and `kai-call 1` requests: cut, mutated or bent
//! at every byte, each is read or refused with an error, never a panic.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use livecore::actions;
use livecore::call::{Call, parse_reply};

const NOTES: &str = "kai-actions 1
world app:notes/notes@1.0.0
action notes_add
export app:notes/actions@1.0.0#add
doc Add a note and return its id.
sig func(text: string) -> result<u32, string>
arg text text max=2000 role=payload from-request
facts read-only=no destructive=no idempotent=no open-world=no
inverse snapshot
end
action notes_find
export app:notes/actions@1.0.0#find
doc Find notes with a word.
type hit = record { id: u32, words: list<option<tuple<u8, s64>>> }
type order = enum { newest, oldest }
type shape = variant { one(u32), none }
type perms = flags { read, write }
sig func(word: string, order: order, shape: shape, perms: perms) -> result<list<hit>>
arg word text max=60 role=target from-request
arg order enum role=option
arg shape int
arg perms int
cues find search look
inverse none
end
";

const CALL: &str =
    "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy \\\"milk\\\" \u{1F600}\"}\n";

fn read(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    if let Ok(parsed) = actions::parse(text) {
        for row in &parsed.rows {
            let _ = row.args_record();
        }
    }
    if let Ok(call) = Call::parse(text) {
        assert_eq!(Call::parse(&call.format()), Ok(call));
    }
    let _ = parse_reply(text);
}

#[test]
fn mutated_sections_and_calls_are_read_or_refused() {
    for (seed, sample) in [(11, NOTES), (12, CALL)] {
        hostile::fuzz(sample.as_bytes(), seed, 20_000, read);
        hostile::truncations(sample.as_bytes(), |_, cut| read(cut));
    }
}

#[test]
fn the_samples_are_valid_or_refused_as_designed() {
    // `shape` and `perms` are variant and flags typed as int: refused.
    assert!(actions::parse(NOTES).is_err());
    let fixed = NOTES.replace("arg shape int\narg perms int\n", "");
    let fixed = fixed.replace(", shape: shape, perms: perms", "");
    assert_eq!(actions::parse(&fixed).unwrap().rows.len(), 2);
    assert!(Call::parse(CALL).is_ok());
}
