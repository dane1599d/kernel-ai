use super::*;
use crate::artifact::{Manifest, append_custom_section, bundle};
use crate::ledger::Event;
use crate::store::{Bytes, Component};
use alloc::sync::Arc;
use alloc::vec;
use kabi::live::{MANIFEST_SECTION, SOURCE_SECTION, kind};

const HOST: u64 = 7;
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn artifact(component: &str, version: &str, interface: &str, pad: usize) -> Vec<u8> {
    let mut module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let manifest = Manifest {
        component: component.into(),
        version: version.into(),
        interface: interface.into(),
        features: "-".into(),
        build: "ab".into(),
        source: format!("src-{version}"),
    };
    append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
    let source = bundle(&[("src/lib.rs", b"fn f() {}\n")]);
    append_custom_section(&mut module, SOURCE_SECTION, &source);
    if pad > 0 {
        append_custom_section(&mut module, "pad", &vec![0; pad]);
    }
    module
}

fn event(kind: u64, label: &str) -> Event {
    Event {
        kind,
        label: label.into(),
        schema: 1,
        blob: vec![0x82, 1, 0x80],
        ..Event::default()
    }
}

/// `notes` with boot version 2 running on host pid 7.
fn running() -> Component {
    let mut notes = Component::new("notes");
    let two = artifact("notes", "2", "notes@1", 0);
    notes.add_version("2", Bytes::shared(two)).unwrap();
    notes.record_fetch(HOST, "2");
    notes.event(HOST, &event(kind::RESTORED, "2")).unwrap();
    notes
}

fn put_bytes(
    notes: &mut Component,
    bytes: Vec<u8>,
    base: &str,
    used: usize,
) -> Result<String, Error> {
    let meta = meta(&bytes)?;
    notes.put(&meta, Arc::new(bytes), base, COMMIT, used)
}

fn put(notes: &mut Component, version: &str) -> Result<String, Error> {
    put_bytes(notes, artifact("notes", version, "notes@1", 0), "2", 0)
}

/// Swaps to `label` and commits it, or rolls it back with `reason`.
fn swap(notes: &mut Component, label: &str, rollback: Option<Reason>) {
    notes.record_fetch(HOST, label);
    notes.event(HOST, &event(kind::BEGIN, label)).unwrap();
    let end = match rollback {
        None => event(kind::COMMIT, label),
        Some(reason) => Event {
            reason: reason as u64,
            ..event(kind::ROLLBACK, label)
        },
    };
    notes.event(HOST, &end).unwrap();
}

fn labels(notes: &Component) -> Vec<&str> {
    notes.labels().collect()
}

#[test]
fn labels_start_at_e1_and_advance_only_on_a_successful_upload() {
    let mut notes = running();
    assert_eq!(
        (notes.next_label(), notes.info().next()),
        ("e1".into(), "e1")
    );
    assert_eq!(put(&mut notes, "e2"), Err(Error::Invalid));
    assert_eq!(notes.next_label(), "e1");
    assert_eq!(put(&mut notes, "e1"), Ok("e1".into()));
    assert_eq!(notes.next_label(), "e2");
    for label in ["e2", "e3", "e4", "e5", "e6"] {
        assert_eq!(put(&mut notes, label), Ok(label.into()));
    }
    // The sixth evicted the oldest, and its label is never handed out again.
    assert_eq!(labels(&notes), ["2", "e2", "e3", "e4", "e5", "e6"]);
    assert_eq!(put(&mut notes, "e1"), Err(Error::Invalid));
    assert_eq!(notes.next_label(), "e7");
    assert_eq!(
        (
            notes.uploads.accepted,
            notes.uploads.rejected,
            notes.uploads.evicted
        ),
        (6, 2, 1)
    );
    let e3 = notes.version("e3").unwrap();
    assert_eq!(
        (e3.origin.clone(), e3.commit(), e3.source.as_str()),
        (
            Origin::Upload {
                seq: 3,
                base: "2".into(),
                commit: COMMIT.into()
            },
            Some(COMMIT),
            "src-e3"
        )
    );
}

#[test]
fn every_rule_refuses_and_leaves_the_store_unchanged() {
    let mut notes = running();
    let wrong = [
        (artifact("other", "e1", "notes@1", 0), "2", COMMIT),
        (artifact("notes", "e1", "notes@2", 0), "2", COMMIT),
        (artifact("notes", "e1", "notes@1", 0), "9", COMMIT),
        (artifact("notes", "e1", "notes@1", 0), "../x", COMMIT),
        (artifact("notes", "e1", "notes@1", 0), "2", "0123ABCDEF"),
        (
            artifact("notes", "e1", "notes@1", 0),
            "2",
            "0123456789ABCDEF0123456789abcdef01234567",
        ),
    ];
    for (bytes, base, commit) in wrong {
        let meta = meta(&bytes).unwrap();
        let result = notes.put(&meta, Arc::new(bytes), base, commit, 0);
        assert_eq!(result, Err(Error::Invalid), "{meta:?} from {base} {commit}");
    }
    assert_eq!(meta(b"not an artifact"), Err(Error::Invalid));
    let huge = artifact("notes", "e1", "notes@1", MAX_UPLOAD);
    assert_eq!(meta(&huge), Err(Error::TooBig));
    assert_eq!(labels(&notes), ["2"]);
    assert_eq!(notes.next_label(), "e1");
    assert_eq!(notes.uploads.rejected, 6);
    // Nothing can be uploaded before a version runs: there is no interface to keep.
    let mut idle = Component::new("notes");
    idle.add_version("2", Bytes::shared(artifact("notes", "2", "notes@1", 0)))
        .unwrap();
    assert_eq!(put(&mut idle, "e1"), Err(Error::Invalid));
}

#[test]
fn eviction_takes_failed_uploads_first_then_the_oldest_and_keeps_what_runs() {
    let mut notes = running();
    for label in ["e1", "e2", "e3", "e4", "e5"] {
        put(&mut notes, label).unwrap();
    }
    swap(&mut notes, "e1", None);
    swap(&mut notes, "e3", Some(Reason::Trap));
    put(&mut notes, "e6").unwrap();
    assert_eq!(labels(&notes), ["2", "e1", "e2", "e4", "e5", "e6"]);
    // e1 is active, so the oldest after it goes.
    put(&mut notes, "e7").unwrap();
    assert_eq!(labels(&notes), ["2", "e1", "e4", "e5", "e6", "e7"]);
    // A candidate on probation is kept too.
    notes.record_fetch(HOST, "e4");
    notes.event(HOST, &event(kind::BEGIN, "e4")).unwrap();
    put(&mut notes, "e8").unwrap();
    assert_eq!(labels(&notes), ["2", "e1", "e4", "e6", "e7", "e8"]);
    assert_eq!(notes.uploads.evicted, 3);
}

#[test]
fn a_store_full_of_versions_that_cannot_be_evicted_is_busy() {
    let mut notes = Component::new("notes");
    for n in 1..=15 {
        let label = format!("{n}");
        let bytes = artifact("notes", &label, "notes@1", 0);
        notes.add_version(&label, Bytes::shared(bytes)).unwrap();
    }
    notes.record_fetch(HOST, "1");
    notes.event(HOST, &event(kind::RESTORED, "1")).unwrap();
    let e1 = artifact("notes", "e1", "notes@1", 0);
    assert_eq!(put_bytes(&mut notes, e1, "1", 0), Ok("e1".into()));
    swap(&mut notes, "e1", None);
    let e2 = artifact("notes", "e2", "notes@1", 0);
    assert_eq!(put_bytes(&mut notes, e2, "1", 0), Err(Error::Busy));
    assert_eq!(notes.next_label(), "e2");
}

#[test]
fn uploads_stay_within_the_total_budget() {
    let mut notes = running();
    let big = |label: &str| artifact("notes", label, "notes@1", 100_000);
    put_bytes(&mut notes, big("e1"), "2", 0).unwrap();
    put_bytes(&mut notes, big("e2"), "2", 0).unwrap();
    let size = big("e3").len();
    assert_eq!(notes.upload_bytes(), 2 * size);
    // Room for two: the oldest makes way.
    let tight = MAX_UPLOAD_TOTAL - 2 * size - 10;
    put_bytes(&mut notes, big("e3"), "2", tight).unwrap();
    assert_eq!(labels(&notes), ["2", "e2", "e3"]);
    // No room at all: refused, and nothing is evicted.
    assert_eq!(
        put_bytes(&mut notes, big("e4"), "2", MAX_UPLOAD_TOTAL),
        Err(Error::NoMemory)
    );
    assert_eq!(labels(&notes), ["2", "e2", "e3"]);
    assert_eq!(notes.next_label(), "e4");
}

#[test]
fn apply_picks_the_newest_upload_unless_it_runs_or_failed() {
    let mut notes = running();
    assert!(
        apply_candidate(&notes)
            .unwrap_err()
            .contains("has no uploads")
    );
    put(&mut notes, "e1").unwrap();
    assert_eq!(apply_candidate(&notes), Ok("e1".into()));
    swap(&mut notes, "e1", None);
    assert_eq!(
        apply_candidate(&notes),
        Err("nothing new: e1 is active".into())
    );
    put(&mut notes, "e2").unwrap();
    swap(&mut notes, "e2", Some(Reason::Trap));
    assert_eq!(
        apply_candidate(&notes),
        Err("e2 rolled back before (trap); :swap notes e2 force retries it".into())
    );
    put(&mut notes, "e3").unwrap();
    notes.record_fetch(HOST, "e3");
    notes.event(HOST, &event(kind::BEGIN, "e3")).unwrap();
    assert_eq!(
        apply_candidate(&notes),
        Err("busy: notes e3 is on probation".into())
    );
}

#[test]
fn the_outbox_records_commits_and_rollbacks_only_while_sync_is_on() {
    let mut notes = running();
    put(&mut notes, "e1").unwrap();
    swap(&mut notes, "e1", None);
    assert_eq!(notes.uploads.outbox.pending(), 0);
    notes.uploads.outbox.enable();
    put(&mut notes, "e2").unwrap();
    swap(&mut notes, "e2", None);
    put(&mut notes, "e3").unwrap();
    swap(&mut notes, "e3", Some(Reason::Trap));
    // Checkpoints, faults and restores are not swaps.
    let checkpoint = Event {
        label: String::new(),
        ..event(kind::CHECKPOINT, "")
    };
    notes.event(HOST, &checkpoint).unwrap();
    let fault = Event {
        reason: Reason::Trap as u64,
        label: String::new(),
        ..event(kind::FAULT, "")
    };
    notes.event(HOST, &fault).unwrap();
    notes.record_fetch(8, "e2");
    notes.event(8, &event(kind::RESTORED, "e2")).unwrap();
    let outbox = &mut notes.uploads.outbox;
    assert_eq!(outbox.pending(), 2);
    let first = outbox.peek().unwrap().clone();
    assert_eq!(
        (
            first.seq,
            first.label.as_str(),
            first.outcome,
            first.generation
        ),
        (1, "e2", Outcome::Commit, 2)
    );
    assert_eq!(
        (first.source.as_str(), first.commit.as_str()),
        ("src-e2", COMMIT)
    );
    // Only the oldest job can be acknowledged, and it stays until it is.
    assert!(!outbox.ack(2, "abc"));
    assert!(outbox.ack(1, "abc"));
    let second = outbox.peek().unwrap();
    assert_eq!(
        (second.seq, second.outcome),
        (2, Outcome::Rollback(Reason::Trap))
    );
    let synced: Vec<_> = outbox.synced().collect();
    assert_eq!((synced.len(), synced[0].sha.as_deref()), (1, Some("abc")));
}

#[test]
fn a_full_outbox_drops_the_oldest_job_and_counts_it() {
    let mut outbox = Outbox::new();
    outbox.enable();
    for n in 0..=OUTBOX {
        let label = format!("e{n}");
        outbox.push(&label, Outcome::Commit, 1, String::new(), String::new());
    }
    assert_eq!((outbox.pending(), outbox.dropped), (OUTBOX, 1));
    assert_eq!(outbox.peek().map(|job| job.label.as_str()), Some("e1"));
}
