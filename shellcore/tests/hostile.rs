//! A hostile `actlog 1` text, as P2 will read one back from disk: cut, mutated or
//! bent at every byte, it is read or refused with an error, never a panic, and
//! what is read is no larger than the text.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use shellcore::actlog::parse;

const SAMPLE: &str = "actlog 1\nE 17 t=1792618800123456789 reg=1 form=call tier=run consent=asked state=done\nR 21\nswitch to round robin\nO 3\nran\nC 26\nset_policy(\"round-robin\");\nD 17.1 set_policy owner=kernel res=policy before=interactive after=round-robin inv=restore\nU 18 of=17 t=1792618860000000000 state=done\nD 18.1 set_policy owner=kernel res=policy before=round-robin after=interactive inv=restore\nS 19 t=1792618860000000000 owner=kernel 33\nnvme0 reset after 5 s timeout, ok\n";

fn read(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let (result, peak) = hostile::peak_alloc(|| parse(text));
    if let Ok(records) = result {
        assert!(records.len() <= text.len());
    }
    assert!(
        peak <= 64 * 1024 + 16 * text.len(),
        "{peak} bytes for {} bytes",
        text.len()
    );
}

#[test]
fn the_sample_reads() {
    assert_eq!(parse(SAMPLE).unwrap().len(), 3);
}

#[test]
fn mutated_logs_are_read_or_refused() {
    hostile::fuzz(SAMPLE.as_bytes(), 8, 30_000, read);
    hostile::truncations(SAMPLE.as_bytes(), |_, cut| read(cut));
}
