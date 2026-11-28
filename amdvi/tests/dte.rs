//! Device table entries: each field at its bits, and what cannot be written
//! is refused.

use amdvi::dte::{DeviceEntry, DteError, ENTRY_BYTES, Paging, table_bytes};

#[path = "../../testkit/hostile.rs"]
mod hostile;
mod kit;

const IR_IW: u64 = 1 << 61 | 1 << 62;

#[test]
fn pass_through_is_valid_untranslated_with_reads_and_writes() {
    let words = DeviceEntry::pass_through(0x0102).encode().unwrap();
    assert_eq!(words, [0b11 | IR_IW, 0x0102, 0, 0]);
    let blocked = DeviceEntry::blocked(3).encode().unwrap();
    assert_eq!(blocked, [0b11, 3, 0, 0]);
}

/// IV (bit 128) stays clear in every entry this crate writes: IV set with
/// no interrupt table would block the device's interrupts, so until
/// interrupt remapping lands they pass as the device sends them.
#[test]
fn no_entry_sets_iv() {
    let mut dma = kit::FakeDma::new();
    let table = amdvi::pagetable::IoPageTable::new(&mut dma, 4).unwrap();
    let entries = [
        DeviceEntry::pass_through(1),
        DeviceEntry::blocked(2),
        DeviceEntry::paged_for(3, &table),
    ];
    for entry in entries {
        let words = entry.encode().unwrap();
        assert_eq!(words[2] & 1, 0, "{entry:?}");
        // Nor any other interrupt field: words 2 and 3 are all zero.
        assert_eq!(words[2..], [0, 0], "{entry:?}");
        assert_eq!(entry.to_bytes().unwrap()[16], 0);
    }
}

#[test]
fn a_paged_entry_holds_its_mode_root_and_domain() {
    let entry = DeviceEntry::paged(7, 4, 0x000f_1234_5000).unwrap();
    let words = entry.encode().unwrap();
    assert_eq!(words[0], 0b11 | 4 << 9 | 0x000f_1234_5000 | IR_IW);
    assert_eq!(words[1], 7);
    assert_eq!(words[2..], [0, 0]);
    assert_eq!(DeviceEntry::decode(words), Ok(entry));
    let bytes = entry.to_bytes().unwrap();
    assert_eq!(bytes.len(), ENTRY_BYTES);
    assert_eq!(bytes[..8], words[0].to_le_bytes());
    assert_eq!(bytes[8..16], 7u64.to_le_bytes());
}

#[test]
fn each_flag_has_its_own_bit() {
    let none = DeviceEntry {
        valid: false,
        translation_valid: false,
        read: false,
        write: false,
        ..DeviceEntry::pass_through(0)
    };
    assert_eq!(none.encode().unwrap(), [0; 4]);
    let one = |set: fn(&mut DeviceEntry)| {
        let mut entry = none;
        set(&mut entry);
        entry.encode().unwrap()
    };
    assert_eq!(one(|e| e.valid = true), [1 << 0, 0, 0, 0]);
    assert_eq!(one(|e| e.translation_valid = true), [1 << 1, 0, 0, 0]);
    assert_eq!(one(|e| e.read = true), [1 << 61, 0, 0, 0]);
    assert_eq!(one(|e| e.write = true), [1 << 62, 0, 0, 0]);
    assert_eq!(one(|e| e.suppress_events = true), [0, 1 << 33, 0, 0]);
    assert_eq!(one(|e| e.suppress_all = true), [0, 1 << 34, 0, 0]);
    assert_eq!(one(|e| e.domain = 0xffff), [0, 0xffff, 0, 0]);
}

#[test]
fn bad_levels_and_roots_are_refused() {
    for levels in [0, 7, 8, 255] {
        assert_eq!(
            DeviceEntry::paged(1, levels, 0x1000),
            Err(DteError::Levels(levels))
        );
    }
    for root in [0x1001, 0x10_0000_0000_0000, !0xfff] {
        assert_eq!(DeviceEntry::paged(1, 4, root), Err(DteError::Root(root)));
    }
    for levels in 1..=6 {
        assert!(DeviceEntry::paged(1, levels, 0x1000).is_ok());
    }
    // Mode 7 is reserved: read back, it is an error, not a seven-level table.
    assert_eq!(
        DeviceEntry::decode([7 << 9, 0, 0, 0]),
        Err(DteError::Levels(7))
    );
}

/// Any words read back give an entry or an error, and an entry writes back
/// as exactly the bits it read.
#[test]
fn any_words_decode_to_what_they_encode() {
    const KNOWN0: u64 = 0b11 | 7 << 9 | 0x000f_ffff_ffff_f000 | IR_IW;
    const KNOWN1: u64 = 0xffff | 0b111 << 32;
    let mut rng = hostile::XorShift::new(0x48882);
    for _ in 0..20_000 {
        let words = [rng.next(), rng.next(), rng.next(), rng.next()];
        let Ok(entry) = DeviceEntry::decode(words) else {
            assert_eq!(words[0] >> 9 & 7, 7);
            continue;
        };
        if entry.iotlb {
            assert_eq!(entry.encode(), Err(DteError::Iotlb));
            continue;
        }
        let again = entry.encode().unwrap();
        let mut low = words[0] & KNOWN0;
        if entry.paging == Paging::Off {
            // Mode 0 has no root.
            low &= !0x000f_ffff_ffff_f000;
        }
        assert_eq!(again, [low, words[1] & KNOWN1, 0, 0]);
        assert_eq!(DeviceEntry::decode(again), Ok(entry));
    }
}

/// The I bit is read but never written: a device IOTLB needs ATS and its
/// own invalidations first.
#[test]
fn an_iotlb_entry_is_refused() {
    let entry = DeviceEntry {
        iotlb: true,
        ..DeviceEntry::pass_through(1)
    };
    assert_eq!(entry.encode(), Err(DteError::Iotlb));
    assert!(DeviceEntry::decode([1, 1 << 32, 0, 0]).unwrap().iotlb);
}

/// An entry made for a page table carries its root and levels.
#[test]
fn an_entry_for_a_page_table_names_its_root_and_levels() {
    let mut dma = kit::FakeDma::new();
    let table = amdvi::pagetable::IoPageTable::new(&mut dma, 4).unwrap();
    let entry = DeviceEntry::paged_for(5, &table);
    assert_eq!(
        entry.paging,
        Paging::Tables {
            levels: 4,
            root: table.root()
        }
    );
    assert_eq!(entry.encode().unwrap()[0] >> 9 & 7, 4);
}

#[test]
fn a_table_is_whole_pages_of_entries() {
    assert_eq!(table_bytes(0), 4096);
    assert_eq!(table_bytes(127), 4096);
    assert_eq!(table_bytes(128), 8192);
    assert_eq!(table_bytes(0xffff), 2 << 20);
}
