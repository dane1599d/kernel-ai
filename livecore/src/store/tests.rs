use super::*;
use kabi::live::kind;

fn blob() -> Vec<u8> {
    alloc::vec![0x82, 1, 0x80]
}

fn event(kind: u64, label: &str) -> Event {
    Event {
        kind,
        label: label.into(),
        schema: 1,
        blob: blob(),
        ..Event::default()
    }
}

#[test]
fn a_fetch_counts_only_when_it_fits_and_lets_the_host_report() {
    let mut notes = Component::new("notes");
    notes
        .add_version("1", Bytes::Static(b"module one"))
        .unwrap();
    assert_eq!(
        notes.add_version("1", Bytes::Static(b"")),
        Err("version listed twice")
    );
    assert_eq!(
        notes.add_version("Bad", Bytes::Static(b"")),
        Err("bad version label")
    );
    assert_eq!(notes.fetch(5, "1", 3), Ok((&b"module one"[..], false)));
    assert_eq!(notes.fetches("1"), 0);
    assert_eq!(notes.fetch(5, "2", 100), Err(Error::NotFound));
    assert_eq!(notes.fetch(5, "../x", 100), Err(Error::Invalid));
    assert!(notes.fetch(5, "1", 100).unwrap().1);
    assert_eq!(notes.fetches("1"), 1);
    assert!(notes.event(5, &event(kind::RESTORED, "1")).is_ok());
    let restore = notes.restore();
    assert_eq!(
        (restore.phase, restore.active(), restore.checkpoint_len),
        (phase::STEADY, "1", 3)
    );
}

#[test]
fn shared_bytes_fetch_and_count_like_boot_modules() {
    let mut notes = Component::new("notes");
    notes
        .add_version("2", Bytes::shared(b"module two".to_vec()))
        .unwrap();
    assert_eq!(notes.fetch(5, "2", 100), Ok((&b"module two"[..], true)));
    assert_eq!(notes.fetches("2"), 1);
    let copy = notes.bytes("2").unwrap().clone();
    assert_eq!(copy.as_slice(), b"module two");
    assert_eq!(
        notes.version("2").map(|v| v.origin.clone()),
        Some(Origin::Boot)
    );
}

#[test]
fn boot_modules_may_not_take_upload_labels() {
    let mut notes = Component::new("notes");
    assert_eq!(
        notes.add_version("e3", Bytes::Static(b"")),
        Err("e and a number is an upload label")
    );
    assert!(notes.add_version("ex", Bytes::Static(b"")).is_ok());
}

#[test]
fn a_version_must_be_stored_to_start_or_be_swapped_to() {
    let mut notes = Component::new("notes");
    notes.add_version("1", Bytes::Static(b"one")).unwrap();
    assert_eq!(
        notes.event(5, &event(kind::RESTORED, "e1")),
        Err(Error::NotFound)
    );
    notes.record_fetch(5, "1");
    notes.event(5, &event(kind::RESTORED, "1")).unwrap();
    // Even a label the host fetched before it was evicted.
    notes.ledger.fetched(5, "e9");
    assert_eq!(
        notes.event(5, &event(kind::BEGIN, "e9")),
        Err(Error::NotFound)
    );
    assert_eq!(notes.ledger.counters.begins, 0);
    assert_eq!(notes.ledger.counters.rejected, 0);
}

#[test]
fn info_reports_the_phase_labels_and_next_upload() {
    let mut notes = Component::new("notes");
    notes.add_version("1", Bytes::Static(b"one")).unwrap();
    let info = notes.info();
    assert_eq!(
        (info.phase, info.active(), info.next(), info.uploads),
        (phase::EMPTY, "", "e1", 0)
    );
    notes.record_fetch(5, "1");
    notes.event(5, &event(kind::RESTORED, "1")).unwrap();
    let info = notes.info();
    assert_eq!((info.phase, info.active()), (phase::STEADY, "1"));
}

/// [`Component::add_package`] of `bytes` checked against `sha`.
fn add(component: &mut Component, bytes: Vec<u8>, sha: &[u8; 32]) -> Result<String, Error> {
    component.add_package(PackageArtifact::check(bytes, sha)?, 0)
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[test]
fn a_package_version_is_labelled_by_its_own_digest_and_only_added() {
    let mut notes = Component::new("notes");
    notes.add_version("2", Bytes::Static(b"boot two")).unwrap();
    let artifact = b"package one".to_vec();
    let sha = digest(&artifact);
    let label = add(&mut notes, artifact.clone(), &sha).unwrap();
    assert_eq!(label, package_label(&sha));
    assert!(is_package_label(&label) && label.len() == 14);
    let version = notes.version(&label).unwrap();
    assert_eq!(version.origin, Origin::Package { sha });
    assert_eq!(notes.fetch(5, &label, 100), Ok((&b"package one"[..], true)));
    // Started like any version, through the ledger.
    assert!(notes.event(5, &event(kind::RESTORED, &label)).is_ok());
    // A second add of the same label replaces nothing, not even with the same bytes.
    assert_eq!(add(&mut notes, artifact, &sha), Err(Error::Busy));
    assert_eq!(notes.bytes(&label).unwrap().as_slice(), b"package one");
    // A boot module may not take a package label.
    assert!(notes.add_version(&label, Bytes::Static(b"x")).is_err());
}

#[test]
fn a_wrong_digest_or_an_oversized_or_unstorable_package_is_refused() {
    let mut notes = Component::new("notes");
    let artifact = b"package one".to_vec();
    let mut wrong = digest(&artifact);
    wrong[31] ^= 1;
    assert_eq!(
        add(&mut notes, artifact.clone(), &wrong),
        Err(Error::Invalid)
    );
    // The digest of other bytes, even with the same first 12 hex digits' label.
    assert_eq!(
        add(&mut notes, b"other".to_vec(), &digest(&artifact)),
        Err(Error::Invalid)
    );
    let big = alloc::vec![0u8; MAX_ARTIFACT + 1];
    assert_eq!(
        add(&mut notes, big.clone(), &digest(&big)),
        Err(Error::TooBig)
    );
    assert_eq!(notes.versions().len(), 0);
    for n in 0..MAX_VERSIONS {
        let bytes = alloc::format!("version {n}").into_bytes();
        add(&mut notes, bytes.clone(), &digest(&bytes)).unwrap();
    }
    let last = b"one too many".to_vec();
    assert_eq!(
        add(&mut notes, last.clone(), &digest(&last)),
        Err(Error::NoMemory)
    );
    assert_eq!(notes.versions().len(), MAX_VERSIONS);
}

#[test]
fn package_versions_of_every_component_together_stay_within_their_budget() {
    // 1 MiB artifacts, told apart by their first byte.
    let artifact = |n: u8| {
        let mut bytes = alloc::vec![0u8; MAX_ARTIFACT];
        bytes[0] = n;
        bytes
    };
    let mut store: Vec<Component> = ["notes", "files", "editor"]
        .into_iter()
        .map(Component::new)
        .collect();
    let used_except = |store: &[Component], at: usize| -> usize {
        let others = store.iter().enumerate().filter(|(i, _)| *i != at);
        others.map(|(_, c)| c.package_bytes()).sum()
    };
    let mut added = 0;
    for n in 0..=u8::MAX {
        let at = usize::from(n) % store.len();
        let bytes = artifact(n);
        let used = used_except(&store, at);
        let checked = PackageArtifact::check(bytes.clone(), &digest(&bytes)).unwrap();
        match store[at].add_package(checked, used) {
            Ok(_) => added += 1,
            Err(error) => {
                assert_eq!(error, Error::NoMemory);
                break;
            }
        }
    }
    assert_eq!(added, MAX_PACKAGE_TOTAL / MAX_ARTIFACT);
    let held: usize = store.iter().map(Component::package_bytes).sum();
    assert!(held <= MAX_PACKAGE_TOTAL, "{held}");
    // A version already there is still Busy, never NoMemory, and uploads do not
    // count against the packages' budget.
    let first = artifact(0);
    let again = PackageArtifact::check(first.clone(), &digest(&first)).unwrap();
    let used = used_except(&store, 0);
    assert_eq!(store[0].add_package(again, used), Err(Error::Busy));
    assert_eq!(store[0].upload_bytes(), 0);
}
