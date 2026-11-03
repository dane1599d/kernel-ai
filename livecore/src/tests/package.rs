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

fn add_package(kernel: &super::Kernel, spec: super::fake::Spec) -> alloc::string::String {
    let artifact = spec.artifact();
    let sha: [u8; 32] = Sha256::digest(&artifact).into();
    let checked = PackageArtifact::check(artifact, &sha).unwrap();
    let mut component = kernel.component.borrow_mut();
    component.add_package(checked, 0).unwrap()
}

#[test]
fn a_host_started_on_another_package_version_swaps_to_it_with_the_state() {
    // pkg ran 0.0.1, an undo withdrew it, and pkg now runs 0.0.2: the ledger
    // still names 0.0.1, but the version pkg names is the one that serves.
    let kernel = kernel(&[]);
    let old = add_package(&kernel, V1);
    let new = add_package(&kernel, super::V2);
    let settings = |label: &str| alloc::format!("version={label} probation_calls=0 probation_ms=0");
    let mut first = host_with(&kernel, 10, &settings(&old));
    call(&mut first, "add");
    drop(first);
    let mut second = host_with(&kernel, 11, &settings(&new));
    assert_eq!(call(&mut second, "get"), "ok 1 schema 2");
    let ledger = ledger(&kernel);
    assert_eq!(
        (ledger.active(), ledger.counters.commits),
        (Some(new.as_str()), 1)
    );
    let log = kernel.log.borrow();
    let said = alloc::format!("counter runs {old} but pkg names {new}: ok committed {new}");
    assert!(log.iter().any(|line| line.contains(&said)), "{log:?}");
}

#[test]
fn a_host_restarted_on_the_same_package_version_restores_it() {
    let kernel = kernel(&[]);
    let label = add_package(&kernel, V1);
    let settings = alloc::format!("version={label} probation_calls=0 probation_ms=0");
    let mut first = host_with(&kernel, 10, &settings);
    call(&mut first, "add");
    drop(first);
    let mut second = host_with(&kernel, 11, &settings);
    assert_eq!(call(&mut second, "get"), "ok 1 schema 1");
    assert_eq!(ledger(&kernel).counters.commits, 0);
}

#[test]
fn a_build_swapped_over_a_package_version_is_kept_on_restart() {
    // Only another package version replaces a package version: a build the
    // person swapped in stays, whatever pkg's label says.
    let kernel = kernel(&[super::V2]);
    let label = add_package(&kernel, V1);
    let settings = alloc::format!("version={label} probation_calls=0 probation_ms=0");
    let mut first = host_with(&kernel, 10, &settings);
    assert_eq!(call(&mut first, "ctl swap 2"), "ok committed 2");
    drop(first);
    let mut second = host_with(&kernel, 11, &settings);
    assert_eq!(call(&mut second, "get"), "ok 0 schema 2");
    assert_eq!(ledger(&kernel).active(), Some("2"));
}

#[test]
fn the_package_version_pkg_names_skips_probation_so_the_withdrawn_one_never_answers() {
    let kernel = kernel(&[]);
    let old = add_package(&kernel, V1);
    let new = add_package(&kernel, super::V2);
    let settings = |label: &str| alloc::format!("version={label}");
    let mut first = host_with(&kernel, 10, &settings(&old));
    call(&mut first, "add");
    drop(first);
    let mut second = host_with(&kernel, 11, &settings(&new));
    assert_eq!(
        call(&mut second, "ctl status"),
        alloc::format!("ok counter {new} schema 2")
    );
    assert_eq!(ledger(&kernel).active(), Some(new.as_str()));
}

#[test]
fn a_package_version_that_fails_its_checks_leaves_the_old_one_serving() {
    let kernel = kernel(&[]);
    let old = add_package(&kernel, V1);
    let new = add_package(&kernel, super::broken("2", super::fake::Bug::SelfTest));
    let settings = |label: &str| alloc::format!("version={label}");
    let mut first = host_with(&kernel, 10, &settings(&old));
    call(&mut first, "add");
    drop(first);
    let mut second = host_with(&kernel, 11, &settings(&new));
    assert_eq!(call(&mut second, "get"), "ok 1 schema 1");
    let ledger = ledger(&kernel);
    assert_eq!(
        (ledger.active(), ledger.counters.rollbacks),
        (Some(old.as_str()), 1)
    );
}
