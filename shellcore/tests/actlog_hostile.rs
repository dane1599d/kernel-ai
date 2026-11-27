//! The `actlog` fuzz target's body (`fuzz/actlog_harness.rs`), run here over
//! truncations and hostile mutations of its samples, so a harness that cannot
//! build, or that panics on its own, fails the host tests without libFuzzer.
//! A log read back from disk is read or refused with an error, never a panic;
//! what is read writes back as a fixed point; `undo` and `log` answer for it;
//! and each call allocates no more than a small multiple of its input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/actlog_harness.rs"]
mod harness;

use shellcore::actlog::{ActLog, parse, parse_setting_aside};

const ALLOC_PER_BYTE: usize = 64;
const ALLOC_FLOOR: usize = 256 * 1024;

fn bounded(bytes: &[u8]) {
    let ((), peak) = hostile::peak_alloc(|| harness::actlog(bytes));
    let cap = ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR;
    assert!(peak <= cap, "{peak} bytes allocated for {}", bytes.len());
}

#[test]
fn the_samples_read_and_write_back_as_they_are() {
    for sample in [harness::V1, harness::V2] {
        let (records, aside) = parse_setting_aside(sample).unwrap();
        assert!(aside.is_empty());
        // The samples are as the shell writes them, so every field is reached.
        assert_eq!(ActLog::from_records(records).text(), sample);
        harness::actlog(sample.as_bytes());
    }
    assert_eq!(parse(harness::V1).unwrap().len(), 3);
    assert_eq!(parse(harness::V2).unwrap().len(), 2);
}

#[test]
fn mutated_logs_are_read_or_refused() {
    for (i, sample) in [harness::V1, harness::V2].iter().enumerate() {
        hostile::fuzz(sample.as_bytes(), 0x6163_746c + i as u64, 20_000, bounded);
        hostile::truncations(sample.as_bytes(), |_, cut| bounded(cut));
    }
}

/// Two samples one after the other, the second's header dropped, so records
/// meet ones of the other version and ids meet again.
#[test]
fn spliced_logs_are_read_or_refused() {
    let tail = harness::V2.split_once('\n').unwrap().1;
    let spliced = format!("{}{tail}", harness::V1);
    bounded(spliced.as_bytes());
    hostile::fuzz(spliced.as_bytes(), 0x7370_6c63, 10_000, bounded);
    let v2 = format!("actlog 2\n{}", spliced.split_once('\n').unwrap().1);
    hostile::fuzz(v2.as_bytes(), 0x7370_6c64, 10_000, bounded);
}

/// A damaged log with one id twice: `undo` would name the newer entry and
/// revert the older one, whose changes it did not name. Refused, with the
/// second's line, whatever records the two are.
#[test]
fn a_log_with_an_id_used_twice_is_refused() {
    let done = "D 5.1 set_policy owner=kernel res=policy before=interactive after=round-robin inv=spec:1:0:2:0\n";
    let entry = |state: &str| {
        format!("E 5 t=1 reg=1 pack=- form=call tier=run consent=asked state={state}\n")
    };
    let system = "S 5 t=1 owner=kernel 2\nok\n";
    let undo = "U 5 of=4 t=1 state=done\n";
    for (first, second) in [
        (entry("nothing"), format!("{}{done}", entry("done"))),
        (format!("{}{done}", entry("done")), entry("nothing")),
        (system.to_string(), format!("{}{done}", entry("done"))),
        (format!("{}{done}", entry("done")), undo.to_string()),
    ] {
        let text = format!("actlog 1\n{first}{second}");
        let line = 2 + first.matches('\n').count();
        let refused = parse(&text).expect_err(&text);
        assert_eq!(
            (refused.line, refused.why),
            (line, "an id is used twice"),
            "{text}"
        );
        bounded(text.as_bytes());
    }
    // An agent's entry set aside still holds its id.
    let text = format!(
        "actlog 2\nE 5 t=1 reg=1 pack=- form=call tier=run consent=asked state=done agent=Bad!\n{done}{}",
        entry("done").replace("\n", " agent=kai\n")
    );
    assert_eq!(
        parse(&text).map_err(|e| e.why),
        Err("an id is used twice"),
        "{text}"
    );
}
