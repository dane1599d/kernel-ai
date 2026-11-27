//! Hostile slot records (plan P6 G33): `KaiBootState` bytes, `kai-slot 1`
//! and `pending` texts, random, mutated, cut and with every field at every
//! extreme, the record also with its CRC rewritten so every mutation reaches
//! the decoder and R's choice. Each is refused or taken as exactly what it
//! says, and no boot R arms from it breaks the rule (`hostile/harness.rs`).

#[path = "hostile/harness.rs"]
mod harness;
#[path = "../../testkit/hostile.rs"]
mod hostile;

use bootslot::{BYTES, Nv, Reason, Record, Slot, crc32, install, read, stage_begin, stage_end};
use hostile::{XorShift, fuzz, sweep_fields, truncations};

/// Writes the record's CRC over its first 60 bytes, where it has 64.
fn recrc(bytes: &mut [u8]) {
    if bytes.len() == BYTES {
        let crc = crc32(&bytes[..60]);
        bytes[60..].copy_from_slice(&crc.to_le_bytes());
    }
}

/// Checks `bytes`, then the same with the CRC rewritten.
fn both(bytes: &[u8]) {
    harness::slot_record(bytes);
    let mut fixed = bytes.to_vec();
    recrc(&mut fixed);
    harness::slot_record(&fixed);
}

/// Records from a few lives: fresh, A committed and B being written, B on
/// trial, and B spent with A committed.
fn records() -> Vec<Record> {
    let mut nv = Nv::default();
    install(&mut nv, 4);
    let fresh = nv.record;
    nv.record.slots[Slot::A as usize].successful = true;
    nv.record.min_release_seq = 4;
    stage_begin(&mut nv, Slot::B);
    let writing = nv.record;
    stage_end(&mut nv, Slot::B, 5);
    let trial = nv.record;
    let mut spent = trial;
    spent.slots[Slot::B as usize].tries = 0;
    spent.slots[Slot::B as usize].reason = Reason::Unbootable;
    spent.armed = Some(Slot::B);
    vec![fresh, writing, trial, spent]
}

#[test]
fn the_samples_read_back_and_keep_the_rule() {
    for record in records() {
        let bytes = record.encode();
        assert_eq!(read(Some(&bytes)), Ok(Some(record)));
        harness::slot_record(&bytes);
    }
}

#[test]
fn random_records_never_mislead_rollback() {
    let mut rng = XorShift::new(0xb007);
    for _ in 0..20_000 {
        let len = if rng.below(4) == 0 {
            rng.below(80)
        } else {
            BYTES
        };
        let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if len == BYTES {
            bytes[..4].copy_from_slice(bootslot::MAGIC);
            bytes[4] = bootslot::VERSION.0;
            bytes[5] = bootslot::VERSION.1;
            // Small values in the slots' fields, so the rule is tested where
            // it matters, not only refused reasons.
            for n in 0..3 {
                let at = 6 + n * 9;
                bytes[at + 1] = rng.below(5) as u8;
                bytes[at + 2] = rng.below(3) as u8;
                bytes[at + 3] = rng.below(4) as u8;
                bytes[at + 4] = rng.below(5) as u8;
                bytes[at + 5..at + 9].copy_from_slice(&(rng.below(8) as u32).to_le_bytes());
            }
            bytes[33] = [0, 1, 2, 3, 0xFF][rng.below(5)];
            bytes[35..39].copy_from_slice(&(rng.below(8) as u32).to_le_bytes());
        }
        both(&bytes);
    }
}

#[test]
fn every_cut_and_every_field_of_a_record() {
    for record in records() {
        let bytes = record.encode();
        truncations(&bytes, |cut, prefix| {
            assert_eq!(read(Some(prefix)).is_ok(), cut == BYTES, "cut at {cut}");
            both(prefix);
        });
        let mut long = bytes.to_vec();
        long.push(0);
        assert!(read(Some(&long)).is_err());
        sweep_fields(&bytes, 0..60, |_, mutated| both(mutated));
        for (i, seed) in [0x51, 0x52].into_iter().enumerate() {
            fuzz(&bytes, seed + i as u64, 2_000, both);
        }
    }
}

#[test]
fn damaged_slot_files_and_pending_take_only_what_they_say() {
    let file = "kai-slot 1\nrelease_seq 7\nepoch 2\nversion 0.1.0\ncommit abc\nchannel dev\n";
    harness::slot_file(file.as_bytes());
    fuzz(file.as_bytes(), 0x5107, 5_000, harness::slot_file);
    truncations(file.as_bytes(), |_, cut| harness::slot_file(cut));
    sweep_fields(file.as_bytes(), 0..file.len(), |_, b| harness::slot_file(b));
    for (i, text) in [
        "snapshot 7 slot A release 2\n",
        "snapshot 18446744073709551615 slot B release 4294967295",
    ]
    .iter()
    .enumerate()
    {
        harness::pending(text.as_bytes());
        fuzz(text.as_bytes(), 0x9e00 + i as u64, 5_000, harness::pending);
        truncations(text.as_bytes(), |_, cut| harness::pending(cut));
        sweep_fields(text.as_bytes(), 0..text.len(), |_, b| harness::pending(b));
    }
}

#[test]
fn a_single_flipped_bit_with_the_old_crc_is_refused() {
    for record in records() {
        let bytes = record.encode();
        for bit in 0..BYTES * 8 {
            let mut flipped = bytes;
            flipped[bit / 8] ^= 1 << (bit % 8);
            assert!(
                read(Some(&flipped)).is_err(),
                "bit {bit} flipped and the record still read"
            );
            // And it still keeps the rule, were it ever taken.
            harness::slot_record(&flipped);
        }
    }
}
