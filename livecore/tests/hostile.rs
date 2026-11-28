//! Hostile manifests: a `kai-manifest 1` or `2` that is cut, mutated or bent at
//! every byte is read or refused with an error, never a panic, and whatever is
//! read writes back as a `kai-manifest 2` that reads the same. And hostile
//! artifacts, by the P8 fuzz target's body for the section readers
//! (`fuzz/livecore_harness.rs`), which libFuzzer runs on GitHub.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/livecore_harness.rs"]
mod harness;

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

#[test]
fn mutated_artifacts_are_read_or_refused() {
    let samples = harness::seeds();
    for (seed, (_, sample)) in (3..).zip(&samples) {
        hostile::fuzz(sample, seed, 20_000, harness::live_sections);
        hostile::truncations(sample, |_, cut| harness::live_sections(cut));
        // Every length and count in the module's first bytes bent.
        hostile::sweep_fields(sample, 0..sample.len().min(64), |_, bent| {
            harness::live_sections(bent)
        });
    }
}

#[test]
fn the_artifact_seeds_read() {
    for lifecycle in [1, 2] {
        let bytes = harness::artifact(lifecycle);
        let parsed = livecore::artifact::parse(&bytes).unwrap();
        assert_eq!(parsed.actions.is_some(), lifecycle == 2);
    }
}

/// Writes [`harness::seeds`] to `$FUZZ_SEED_DIR/live_sections/`, for
/// `.github/workflows/fuzz.yml` to run before libFuzzer.
#[test]
#[ignore = "writes the fuzz corpus seeds; fuzz.yml runs it"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    for (n, (target, bytes)) in harness::seeds().into_iter().enumerate() {
        let folder = std::path::Path::new(&dir).join(target);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("seed-{n}")), bytes).unwrap();
    }
}
