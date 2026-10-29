//! A hostile `kai-grants 1` file, as one read back from disk: cut, mutated or
//! bent at every byte, it is read or refused with an error, never a panic, and
//! what is read writes back as the same text.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kaigrants::parse;

const SAMPLE: &str = "kai-grants 1\ngrant 41 app=notes kind=dir object=/home/user/notes rights=read+write+enumerate from=picker life=persistent signer=- made=2026-11-02T10:14:00Z\ngrant 42 app=mail kind=host object=mail.example.org rights=connect from=request:9 life=session signer=9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f made=2026-11-02T10:15:00Z\nrule 57 action=proc_stop target=build-3 from=prompt:212 made=2026-11-02T10:20:00Z\nrule 58 action=service_stop target=- from=prompt:213 made=2026-11-02T10:21:00Z\n";

fn read(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let (result, peak) = hostile::peak_alloc(|| parse(text));
    if let Ok(grants) = result {
        assert!(grants.grants.len() + grants.rules.len() <= text.len());
        // What reads is canonical: it writes back byte for byte.
        assert_eq!(grants.text().as_deref(), Ok(text));
    }
    assert!(
        peak <= 16 * 1024 + 16 * text.len(),
        "{peak} bytes for {} bytes",
        text.len()
    );
}

#[test]
fn the_sample_reads() {
    let grants = parse(SAMPLE).unwrap();
    assert_eq!((grants.grants.len(), grants.rules.len()), (2, 2));
}

#[test]
fn mutated_files_are_read_or_refused() {
    hostile::fuzz(SAMPLE.as_bytes(), 8, 30_000, read);
    hostile::truncations(SAMPLE.as_bytes(), |_, cut| read(cut));
}
