//! IVRS (plan P6 G61) over QEMU's own tables, captured on a runner from
//! `-device amd-iommu` (see `fixtures/ivrs/README.md`), and over hostile
//! mutations of them: cut, every field at every extreme value, and thousands
//! of random edits with the length and checksum set again so they reach the
//! blocks.

#[path = "hostile/ivrs.rs"]
mod body;
#[path = "../../testkit/hostile.rs"]
mod hostile;

use acpitables::ivrs::{self, Devices, Kind, Special};
use body::{ALLOC_FLOOR, ALLOC_PER_BYTE, ivrs_table, reseal};
use hostile::{XorShift, fuzz, mutate, peak_alloc, sweep_fields, truncations};

/// Each captured table: its variant's name and its bytes.
const FIXTURES: &[(&str, &[u8])] = &[
    ("q35", include_bytes!("fixtures/ivrs/q35.bin")),
    (
        "q35-devices",
        include_bytes!("fixtures/ivrs/q35-devices.bin"),
    ),
    ("q35-xtsup", include_bytes!("fixtures/ivrs/q35-xtsup.bin")),
];

#[test]
fn every_captured_table_reads() {
    for (name, bytes) in FIXTURES {
        let ivrs = ivrs_table(bytes).unwrap_or_else(|| panic!("{name}: {:?}", ivrs::parse(bytes)));
        assert!(!ivrs.blocks.is_empty(), "{name}");
        let iommus = ivrs.iommus();
        assert_eq!(iommus.len(), 1, "{name}: QEMU has one amd-iommu");
        let iommu = iommus[0];
        assert!(iommu.base != 0, "{name}");
        assert!(!iommu.devices.is_empty(), "{name}: no devices covered");
        // The IOMMU's own function is covered.
        assert!(iommu.device(iommu.device_id).is_some(), "{name}");
    }
}

/// Mutations with the header set again, so the blocks are what changes.
#[test]
fn mutated_tables_never_panic_or_overallocate() {
    for (i, (name, bytes)) in FIXTURES.iter().enumerate() {
        let mut rng = XorShift::new(0x1f25 + i as u64);
        for _ in 0..20_000 {
            let mut table = mutate(&mut rng, bytes);
            if rng.below(4) != 0 {
                reseal(&mut table);
            }
            let ((), peak) = peak_alloc(|| {
                ivrs_table(&table);
            });
            assert!(
                peak <= ALLOC_FLOOR + ALLOC_PER_BYTE * table.len(),
                "{name}: {peak} bytes for a {}-byte table",
                table.len()
            );
        }
        fuzz(bytes, 0x1f26 + i as u64, 2_000, |t| {
            ivrs_table(t);
        });
    }
}

#[test]
fn every_field_at_every_extreme_reads_or_is_refused() {
    for (name, bytes) in FIXTURES {
        sweep_fields(bytes, 0..bytes.len(), |_, t| {
            let mut t = t.to_vec();
            reseal(&mut t);
            ivrs_table(&t);
        });
        assert!(ivrs::parse(bytes).is_ok(), "{name}");
    }
}

#[test]
fn every_cut_reads_or_is_refused() {
    for (name, bytes) in FIXTURES {
        truncations(bytes, |cut, t| {
            // As cut, the length no longer matches: refused.
            assert!(
                cut == bytes.len() || ivrs::parse(t).is_err(),
                "{name} at {cut}"
            );
            let mut t = t.to_vec();
            reseal(&mut t);
            ivrs_table(&t);
        });
    }
}

#[test]
fn the_highest_type_is_the_one_kept() {
    for (name, bytes) in FIXTURES {
        let ivrs = ivrs::parse(bytes).unwrap();
        let highest = ivrs.blocks.iter().map(|b| b.kind).max().unwrap();
        assert_eq!(ivrs.iommus()[0].kind, highest, "{name}");
        if ivrs.blocks.iter().any(|b| b.kind != Kind::Fixed10) {
            assert_ne!(ivrs.iommus()[0].efr, 0, "{name}: no EFR image");
        }
    }
}

/// What QEMU's tables hold (`fixtures/ivrs/README.md`): one IOMMU at 00:02.0,
/// described by a 10h and an 11h block, its I/O APIC, and the bridge's alias.
#[test]
fn the_captured_tables_hold_what_qemu_builds() {
    for (name, bytes) in FIXTURES {
        let ivrs = ivrs::parse(bytes).unwrap();
        let kinds: Vec<Kind> = ivrs.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, [Kind::Fixed10, Kind::Efr11], "{name}");
        let iommu = ivrs.iommus()[0];
        assert_eq!(iommu.device_id, 0x0010, "{name}: 00:02.0");
        assert_eq!(iommu.base, 0xfed8_0000, "{name}");
        assert_eq!(iommu.capability, 0x40, "{name}");
        assert_eq!(
            iommu.specials,
            [Special {
                setting: 0,
                handle: 0,
                source: 0x00a0,
                variety: 1,
            }],
            "{name}: the I/O APIC, as 00:14.0"
        );
        // Both blocks list the same devices.
        assert_eq!(ivrs.blocks[0].devices, ivrs.blocks[1].devices, "{name}");
        // XTSup is EFR bit 2.
        assert_eq!(iommu.efr & 4 != 0, *name == "q35-xtsup", "{name}");
    }
    let devices = ivrs::parse(FIXTURES[1].1).unwrap();
    let iommu = devices.iommus()[0];
    let root_port = iommu.device(0x0150).unwrap();
    assert_eq!((root_port.first, root_port.last), (0x0100, 0x01ff));
    assert_eq!(
        iommu.device(0x0234),
        Some(Devices {
            first: 0x0200,
            last: 0x02ff,
            setting: 0,
            alias: Some(0x0028),
            extended: 0,
        }),
        "the bridge's bus arrives as the bridge, 00:05.0"
    );
    assert_eq!(iommu.device(0x0300), None);
}

#[test]
fn a_damaged_table_says_what_it_dropped() {
    for (name, bytes) in FIXTURES {
        let whole = ivrs_table(bytes).unwrap();
        assert_eq!(whole.dropped, 0, "{name}: a table read whole drops nothing");
        assert_eq!(whole.physical_address_bits(), 40, "{name}");
        assert_eq!(whole.info, 0x2801, "{name}");
        assert_eq!(whole.virtual_address_bits(), 0, "{name}");
        assert_eq!(whole.guest_virtual_size(), 0, "{name}");
        // A second IVHD whose length runs past the table: the first IOMMU is
        // read, and the loss is counted, not silent.
        let mut cut = bytes.to_vec();
        let last = block_starts(&cut).pop().unwrap();
        let len = cut.len() - last + 8;
        cut[last + 2..last + 4].copy_from_slice(&(len as u16).to_le_bytes());
        body::reseal(&mut cut);
        let read = ivrs_table(&cut).unwrap();
        assert!(read.dropped >= 1, "{name}");
        assert_eq!(read.blocks.len(), whole.blocks.len() - 1, "{name}");
    }
}

/// Where each block of `table` starts.
fn block_starts(table: &[u8]) -> Vec<usize> {
    let mut at = 48;
    let mut out = Vec::new();
    while at + 4 <= table.len() {
        out.push(at);
        at += usize::from(u16::from_le_bytes([table[at + 2], table[at + 3]]));
    }
    out
}

/// A table of many blocks with distinct IDs: `iommus` sorts, it does not
/// compare each pair (a 1 MiB table holds about 43,000 24-byte blocks).
#[test]
fn many_blocks_are_not_compared_pairwise() {
    let mut table = vec![0u8; 48];
    table[..4].copy_from_slice(b"IVRS");
    for id in (0..40_000u16).rev() {
        let mut block = [0u8; 24];
        block[0] = 0x10;
        block[2..4].copy_from_slice(&24u16.to_le_bytes());
        block[4..6].copy_from_slice(&id.to_le_bytes());
        table.extend_from_slice(&block);
    }
    body::reseal(&mut table);
    let ivrs = ivrs::parse(&table).unwrap();
    let iommus = ivrs.iommus();
    assert_eq!(iommus.len(), 40_000);
    assert!(iommus.windows(2).all(|w| w[0].device_id < w[1].device_id));
}
