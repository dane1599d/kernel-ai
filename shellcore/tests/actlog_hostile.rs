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
