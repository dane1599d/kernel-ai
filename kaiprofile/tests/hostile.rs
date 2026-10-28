//! A hostile `kai-profile 1` text, as a user might write one or one comes back
//! from disk: cut, mutated or bent at every byte, it is read or refused, never a
//! panic, and what reads writes back byte for byte.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kaiprofile::parse;

const SAMPLE: &str = "kai-profile 1\nname gaming\napp notes class=normal mem=2048 gpu=0\nfocused class=normal mem=- gpu=0\ngroup builds members=build-*+indexer class=background mem=65536 gpu=0\nothers class=background mem=- gpu=0\n";

fn read(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let (result, peak) = hostile::peak_alloc(|| parse(text));
    if let Ok(profile) = result {
        assert_eq!(profile.text(), text);
        // Resolving what reads never panics either.
        let _ = profile.resolve(&["notes", "build-1", "game"], Some("game"));
    }
    assert!(
        peak <= 16 * 1024 + 16 * text.len(),
        "{peak} bytes for {} bytes",
        text.len()
    );
}

#[test]
fn the_sample_reads() {
    assert_eq!(parse(SAMPLE).unwrap().lines.len(), 4);
}

#[test]
fn mutated_profiles_are_read_or_refused() {
    hostile::fuzz(SAMPLE.as_bytes(), 8, 30_000, read);
    hostile::truncations(SAMPLE.as_bytes(), |_, cut| read(cut));
}
