//! A version from an installed package (P4 task 1) starts like any other: its
//! label is its artifact's digest, which the store checked when `pkg` added it,
//! while its manifest still names the version it was built as.

use super::{V1, call, host_with, kernel, ledger};
use crate::store::PackageArtifact;
use sha2::{Digest, Sha256};

#[test]
fn a_package_version_starts_under_its_digests_label() {
    let kernel = kernel(&[]);
    let artifact = V1.artifact();
    let sha: [u8; 32] = Sha256::digest(&artifact).into();
    let checked = PackageArtifact::check(artifact, &sha).unwrap();
    let label = kernel
        .component
        .borrow_mut()
        .add_package(checked, 0)
        .unwrap();
    let settings = alloc::format!("version={label} probation_calls=3 probation_ms=0");
    let mut host = host_with(&kernel, 10, &settings);
    assert_eq!(call(&mut host, "get"), "ok 0 schema 1");
    assert_eq!(ledger(&kernel).active(), Some(label.as_str()));
}

#[test]
fn any_other_label_must_still_be_its_manifests_own() {
    // An artifact built as version 1, stored as c-0123456789ab: refused, so the
    // host falls back to 1 and says why.
    let mislabelled = super::fake::Spec {
        label: "c-0123456789ab",
        ..V1
    };
    let kernel = kernel(&[V1, mislabelled]);
    let component = kernel.component.borrow();
    let bytes = component.bytes("c-0123456789ab").unwrap().clone();
    drop(component);
    // The same bytes under a label other than their manifest's are refused.
    kernel
        .component
        .borrow_mut()
        .add_version("2", bytes)
        .unwrap();
    let mut host = host_with(&kernel, 10, "version=2 fallback=1");
    assert_eq!(call(&mut host, "get"), "ok 0 schema 1");
    let log = kernel.log.borrow();
    let refused = log
        .iter()
        .any(|line| line.contains("the artifact is counter c-0123456789ab, not counter 2"));
    assert!(refused, "{log:?}");
}
