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
