//! The `slot_record` fuzz targets' bodies (plan P6 G33), which
//! `bootslot/tests/hostile.rs` also runs over hostile records, the way
//! `fuzz/harness.rs` serves its targets: a later
//! `fuzz/fuzz_targets/slot_record.rs` includes this file by `#[path]` and
//! calls these with libFuzzer's input. Each must return for any input, and
//! only a bug in `bootslot` may panic.
//!
//! What a damaged `KaiBootState`, `kai-slot 1` or `pending` must never do is
//! mislead A/B rollback: R never arms a slot the rule forbids (one being
//! written, one that spent its tries, one below the floors), and a record
//! that is taken is one its own encoding writes back.

#![allow(dead_code)]

use bootslot::pending::Pending;
use bootslot::{InR, Nv, Record, Slot, Start, boot, may_boot, r_boots, read, slotfile, start};

/// `KaiBootState` as the firmware gave it: a record that decodes is one that
/// re-encodes to bytes decoding to the same record, and every boot R arms
/// from it, up to a few in a row, keeps the rule.
pub fn slot_record(data: &[u8]) {
    let Ok(Some(record)) = read(Some(data)) else {
        return;
    };
    assert_eq!(data.len(), bootslot::BYTES);
    assert_eq!(Record::decode(&record.encode()), Some(record));
    for slot in Slot::ALL {
        match start(slot, Some(record)) {
            Start::RBoots(r) | Start::Slot(r) => assert_eq!(r, record),
            other => panic!("{other:?} for a record"),
        }
    }
    let mut nv = Nv {
        record,
        one_shot: None,
    };
    // R boots, arms, the armed slot fails before it comes up, R boots again.
    for _ in 0..4 {
        match r_boots(&mut nv) {
            InR::Stay => {
                assert_eq!(nv.one_shot, None);
                assert_eq!(boot(&mut nv), Slot::R);
                break;
            }
            InR::Arm(slot) => {
                assert_ne!(slot, Slot::R);
                assert!(
                    may_boot(&nv.record, slot),
                    "armed {slot:?}: {:?}",
                    nv.record
                );
                assert_eq!(nv.one_shot, Some(slot));
                assert_eq!(nv.record.armed, Some(slot));
                assert_eq!(boot(&mut nv), slot);
                // The boot never finished: the firmware comes back to R.
                assert_eq!(boot(&mut nv), Slot::R);
            }
        }
    }
}

/// `kai-slot 1`: a record taken has the header on its first line, and the
/// release and epoch it gives are those a `key value` line of it says.
pub fn slot_file(data: &[u8]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Some(file) = slotfile::parse(text) else {
        return;
    };
    assert_eq!(
        text.lines().next().map(str::trim_end),
        Some(slotfile::HEADER)
    );
    let said = |key: &str| {
        text.lines()
            .skip(1)
            .filter_map(|l| l.trim_end().split_once(' '))
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .last()
    };
    assert_eq!(
        said("release_seq").and_then(|v| v.parse().ok()),
        Some(file.release_seq)
    );
    assert_eq!(said("epoch").and_then(|v| v.parse().ok()), Some(file.epoch));
}

/// `/system/updates/pending`: an update taken is in A or B, never R, and its
/// own text reads back as it.
pub fn pending(data: &[u8]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let Some(p) = Pending::parse(text) else {
        return;
    };
    assert_ne!(p.slot, Slot::R);
    assert_eq!(Pending::parse(&p.to_string()), Some(p));
}
