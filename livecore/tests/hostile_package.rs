//! Hostile package versions (P4 task 1): `pkg` adds an artifact with a SHA-256
//! the store checks itself. A mutated artifact under the original's digest, a
//! wrong digest, an oversized artifact or a second add of a label is refused,
//! never a panic, and every version that is added is labelled by the digest of
//! the bytes it holds.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use kabi::Error;
use kabi::live::{MAX_ARTIFACT, package_label};
use livecore::store::{Component, Origin, PackageArtifact};
use sha2::{Digest, Sha256};

/// [`Component::add_package`] of `bytes` checked against `sha`.
fn add(component: &mut Component, bytes: Vec<u8>, sha: &[u8; 32]) -> Result<String, Error> {
    component.add_package(PackageArtifact::check(bytes, sha)?, 0)
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Every package version holds bytes whose digest its label and origin name.
fn holds_what_it_names(notes: &Component) {
    for version in notes.versions() {
        let Origin::Package { sha } = version.origin else {
            continue;
        };
        assert_eq!(digest(version.bytes.as_slice()), sha);
        assert_eq!(version.label, package_label(&sha));
    }
}

#[test]
fn mutated_artifacts_and_digests_never_add_what_they_do_not_name() {
    const SAMPLE: &[u8] = b"\0asm\x01\0\0\0 a package's artifact, mutated";
    let original = digest(SAMPLE);
    let mut notes = Component::new("notes");
    hostile::fuzz(SAMPLE, 0x7067, 2_000, |bytes| {
        // Under the original's digest, only the original bytes are taken.
        match add(&mut notes, bytes.to_vec(), &original) {
            Ok(_) => assert_eq!(bytes, SAMPLE),
            Err(e) => assert!(matches!(e, Error::Invalid | Error::Busy | Error::NoMemory)),
        }
        // Under its own digest it is taken once, then Busy for good.
        if let Ok(label) = add(&mut notes, bytes.to_vec(), &digest(bytes)) {
            let again = add(&mut notes, bytes.to_vec(), &digest(bytes));
            assert_eq!(again, Err(Error::Busy), "{label}");
        }
        holds_what_it_names(&notes);
    });
    // Each digest with one bit flipped, against the original bytes.
    let mut fresh = Component::new("notes");
    for bit in 0..256 {
        let mut wrong = original;
        wrong[bit / 8] ^= 1 << (bit % 8);
        assert_eq!(
            add(&mut fresh, SAMPLE.to_vec(), &wrong),
            Err(Error::Invalid)
        );
    }
    assert!(fresh.versions().is_empty());
    // Oversized, whatever its digest says.
    let big = vec![7u8; MAX_ARTIFACT + 1];
    assert_eq!(
        add(&mut fresh, big.clone(), &digest(&big)),
        Err(Error::TooBig)
    );
    holds_what_it_names(&notes);
}
