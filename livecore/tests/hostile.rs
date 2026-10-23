//! Hostile manifests: a `kai-manifest 1` or `2` that is cut, mutated or bent at
//! every byte is read or refused with an error, never a panic, and whatever is
//! read writes back as a `kai-manifest 2` that reads the same.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use livecore::manifest::{Manifest, World, compatible};

const V2: &str = "kai-manifest 2\nlifecycle 1\ncomponent notes\nversion 3\nworld app:notes/notes@1.0.0\nengine wasmi\nfeatures -\nlicence MIT OR Apache-2.0\nbuild 0123456789abcdef\nsource fedcba9876543210\n";
const V1: &str = "kai-manifest 1\nlifecycle 1\ncomponent notes\nversion 3\ninterface notes@1\nfeatures -\nbuild 0123456789abcdef\nsource fedcba9876543210\n";

fn read(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    if let Ok(manifest) = Manifest::parse(text) {
        assert_eq!(Manifest::parse(&manifest.format()), Ok(manifest.clone()));
        let _ = compatible(&manifest.world, V2);
    }
    let _ = World::parse(text);
}

#[test]
fn mutated_manifests_are_read_or_refused() {
    for (seed, sample) in [(1, V2), (2, V1)] {
        hostile::fuzz(sample.as_bytes(), seed, 20_000, read);
        hostile::truncations(sample.as_bytes(), |_, cut| read(cut));
    }
}

#[test]
fn both_samples_read() {
    assert!(Manifest::parse(V2).is_ok());
    assert!(Manifest::parse(V1).is_ok());
}
