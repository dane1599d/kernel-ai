use super::*;
use alloc::vec;

/// An IVRS table holding `blocks`, its length and checksum set.
pub(super) fn table(info: u32, blocks: &[Vec<u8>]) -> Vec<u8> {
    let mut t = vec![0u8; FIRST_BLOCK];
    t[..4].copy_from_slice(b"IVRS");
    t[8] = 2;
    t[SDT_HEADER_LEN..SDT_HEADER_LEN + 4].copy_from_slice(&info.to_le_bytes());
    for block in blocks {
        t.extend_from_slice(block);
    }
    seal(&mut t);
    t
}

/// Sets the table's length and checksum.
fn seal(t: &mut [u8]) {
    let len = t.len() as u32;
    t[4..8].copy_from_slice(&len.to_le_bytes());
    t[9] = 0;
    let sum = t.iter().fold(0u8, |s, &b| s.wrapping_add(b));
    t[9] = sum.wrapping_neg();
}

/// An IVHD block of `kind` for the IOMMU at 00:02.0, base 0xfed80000, with
/// `entries` after its fixed fields.
fn block(kind: u8, entries: &[u8]) -> Vec<u8> {
    let fixed = if kind == 0x10 { IVHD10_LEN } else { IVHD11_LEN };
    let mut b = vec![0u8; fixed];
    b[0] = kind;
    b[1] = 0xb0;
    b[4..6].copy_from_slice(&0x0010u16.to_le_bytes());
    b[6..8].copy_from_slice(&0x40u16.to_le_bytes());
    b[8..16].copy_from_slice(&0xfed8_0000u64.to_le_bytes());
    b[18..20].copy_from_slice(&0x0100u16.to_le_bytes());
    b[20..24].copy_from_slice(&0x1234u32.to_le_bytes());
    if fixed == IVHD11_LEN {
        b[24..32].copy_from_slice(&0x0123_4567_89ab_cdefu64.to_le_bytes());
    }
    b.extend_from_slice(entries);
    let len = b.len() as u16;
    b[2..4].copy_from_slice(&len.to_le_bytes());
    b
}

fn four(kind: u8, id: u16, setting: u8) -> [u8; 4] {
    let id = id.to_le_bytes();
    [kind, id[0], id[1], setting]
}

fn eight(kind: u8, id: u16, setting: u8, tail: [u8; 4]) -> [u8; 8] {
    let id = id.to_le_bytes();
    [
        kind, id[0], id[1], setting, tail[0], tail[1], tail[2], tail[3],
    ]
}

fn alias(source: u16) -> [u8; 4] {
    let s = source.to_le_bytes();
    [0, s[0], s[1], 0]
}

fn range(first: u16, last: u16, setting: u8) -> Devices {
    Devices {
        first,
        last,
        setting,
        alias: None,
        extended: 0,
    }
}

#[test]
fn reads_each_block_type_and_its_fixed_fields() {
    let t = table(
        0x0020_3040,
        &[block(0x10, &[]), block(0x11, &[]), block(0x40, &[])],
    );
    let ivrs = parse(&t).unwrap();
    assert_eq!(ivrs.info, 0x0020_3040);
    let kinds: Vec<Kind> = ivrs.blocks.iter().map(|b| b.kind).collect();
    assert_eq!(kinds, [Kind::Fixed10, Kind::Efr11, Kind::Acpi40]);
    for b in &ivrs.blocks {
        assert_eq!((b.flags, b.device_id, b.capability), (0xb0, 0x10, 0x40));
        assert_eq!((b.base, b.segment, b.iommu_info), (0xfed8_0000, 0, 0x100));
        assert_eq!(b.attributes, 0x1234);
    }
    assert_eq!(ivrs.blocks[0].efr, 0);
    assert_eq!(ivrs.blocks[1].efr, 0x0123_4567_89ab_cdef);
    // One IOMMU described three times: the highest type is the one to use.
    let iommus = ivrs.iommus();
    assert_eq!(iommus.len(), 1);
    assert_eq!(iommus[0].kind, Kind::Acpi40);
}

#[test]
fn reads_every_device_entry() {
    let mut e = Vec::new();
    e.extend(four(0x01, 0, 0x00));
    e.extend(four(0x02, 0x0008, 0x01));
    e.extend(four(0x03, 0x0100, 0x02));
    e.extend(four(0x04, 0x01ff, 0));
    e.extend(eight(0x42, 0x0210, 0x03, alias(0x0200)));
    e.extend(eight(0x43, 0x0300, 0x04, alias(0x0208)));
    e.extend(four(0x04, 0x03ff, 0));
    e.extend(eight(0x46, 0x0400, 0x05, 1u32.to_le_bytes()));
    e.extend(eight(0x47, 0x0500, 0x06, 1u32.to_le_bytes()));
    e.extend(four(0x04, 0x05ff, 0));
    // An I/O APIC with ID 0 as 00:14.0, and an HPET with ID 0 as 00:14.0.
    e.extend(eight(0x48, 0, 0xd7, [0, 0xa0, 0x00, 1]));
    e.extend(eight(0x48, 0, 0x00, [0, 0xa0, 0x00, 2]));
    e.extend(four(0x00, 0, 0));
    let mut acpi = vec![0xf0, 0xa5, 0x00, 0x40];
    acpi.extend(b"AMDI0020");
    acpi.extend(b"\0\0\0\0\0\0\0\0");
    acpi.extend([2, 4]);
    acpi.extend(b"ID00");
    e.extend(&acpi);
    let ivrs = parse(&table(0, &[block(0x40, &e)])).unwrap();
    let b = &ivrs.blocks[0];
    assert_eq!(
        b.devices,
        [
            range(0, u16::MAX, 0),
            range(8, 8, 1),
            range(0x100, 0x1ff, 2),
            Devices {
                alias: Some(0x200),
                ..range(0x210, 0x210, 3)
            },
            Devices {
                alias: Some(0x208),
                ..range(0x300, 0x3ff, 4)
            },
            Devices {
                extended: 1,
                ..range(0x400, 0x400, 5)
            },
            Devices {
                extended: 1,
                ..range(0x500, 0x5ff, 6)
            },
        ]
    );
    assert_eq!(
        b.specials,
        [
            Special {
                setting: 0xd7,
                handle: 0,
                source: 0xa0,
                variety: 1
            },
            Special {
                setting: 0,
                handle: 0,
                source: 0xa0,
                variety: 2
            },
        ]
    );
    assert_eq!(b.acpi.len(), 1);
    let a = b.acpi[0];
    assert_eq!((a.device_id, a.setting, &a.hid), (0xa5, 0x40, b"AMDI0020"));
    assert_eq!(
        (a.uid_format, &a.uid[..usize::from(a.uid_len)]),
        (2, &b"ID00"[..])
    );
    // The last entry that covers a device wins over the All before it.
    assert_eq!(b.device(8), Some(range(8, 8, 1)));
    assert_eq!(b.device(0x150), Some(range(0x100, 0x1ff, 2)));
    assert_eq!(b.device(0x9000), Some(range(0, u16::MAX, 0)));
}

#[test]
fn a_broken_range_is_dropped_and_the_rest_read() {
    let mut e = Vec::new();
    // An end with no start.
    e.extend(four(0x04, 0x10, 0));
    // A start that another start follows: the first is dropped.
    e.extend(four(0x03, 0x20, 1));
    e.extend(four(0x03, 0x30, 2));
    e.extend(four(0x04, 0x3f, 0));
    // An end below its start.
    e.extend(four(0x03, 0x50, 3));
    e.extend(four(0x04, 0x40, 0));
    // Overlapping ranges are kept as firmware gives them; the later wins.
    e.extend(four(0x03, 0x60, 4));
    e.extend(four(0x04, 0x7f, 0));
    e.extend(four(0x03, 0x70, 5));
    e.extend(four(0x04, 0x8f, 0));
    // A start the block's end follows.
    e.extend(four(0x03, 0x90, 6));
    let ivrs = parse(&table(0, &[block(0x11, &e)])).unwrap();
    let b = &ivrs.blocks[0];
    assert_eq!(
        b.devices,
        [
            range(0x30, 0x3f, 2),
            range(0x60, 0x7f, 4),
            range(0x70, 0x8f, 5)
        ]
    );
    assert_eq!(b.device(0x75).map(|d| d.setting), Some(5));
    assert_eq!(b.device(0x65).map(|d| d.setting), Some(4));
    assert_eq!(b.device(0x20), None);
}

#[test]
fn unknown_types_are_skipped_by_their_length() {
    let mut e = Vec::new();
    e.extend(four(0x3f, 0x01, 0xff));
    e.extend(eight(0x7f, 0x02, 0xff, [0xff; 4]));
    e.extend(four(0x02, 0x08, 1));
    // F0h outside a 40h block: skipped by its length.
    let mut acpi = vec![0xf0, 0, 0, 0];
    acpi.extend([0u8; 16]);
    acpi.extend([1, 2, 9, 9]);
    e.extend(&acpi);
    e.extend(four(0x02, 0x10, 2));
    // An IVMD block and an unknown block type between two IVHDs.
    let mut ivmd = vec![0x21, 0, 32, 0];
    ivmd.extend([0u8; 28]);
    let mut other = vec![0x99, 0, 8, 0];
    other.extend([0xee; 4]);
    let t = table(
        0,
        &[block(0x11, &e), ivmd, other, block(0x10, &four(0x02, 3, 0))],
    );
    let ivrs = parse(&t).unwrap();
    assert_eq!(ivrs.blocks.len(), 2);
    assert_eq!(
        ivrs.blocks[0].devices,
        [range(8, 8, 1), range(0x10, 0x10, 2)]
    );
    assert!(ivrs.blocks[0].acpi.is_empty());
    assert_eq!(ivrs.blocks[1].devices, [range(3, 3, 0)]);
}

#[test]
fn an_undefined_variable_entry_ends_its_block_only() {
    let mut e = Vec::new();
    e.extend(four(0x02, 0x08, 1));
    e.extend([0x80, 0, 0, 0]);
    e.extend(four(0x02, 0x10, 2));
    let t = table(0, &[block(0x11, &e), block(0x10, &four(0x02, 3, 0))]);
    let ivrs = parse(&t).unwrap();
    assert_eq!(ivrs.blocks[0].devices, [range(8, 8, 1)]);
    assert_eq!(ivrs.blocks[1].devices, [range(3, 3, 0)]);
}

#[test]
fn short_lengths_never_read_past_the_table() {
    // A block whose length is past the table's end ends the walk.
    let mut long = block(0x11, &four(0x02, 1, 0));
    long[2..4].copy_from_slice(&0xffffu16.to_le_bytes());
    assert!(parse(&table(0, &[long])).unwrap().blocks.is_empty());
    // A length below 4 ends the walk; one below the fixed fields skips it.
    let mut zero = block(0x11, &[]);
    zero[2..4].copy_from_slice(&0u16.to_le_bytes());
    assert!(parse(&table(0, &[zero])).unwrap().blocks.is_empty());
    let mut short = block(0x11, &[]);
    short.truncate(IVHD10_LEN);
    short[2..4].copy_from_slice(&(IVHD10_LEN as u16).to_le_bytes());
    let t = table(0, &[short, block(0x10, &[])]);
    assert_eq!(parse(&t).unwrap().blocks.len(), 1);
    // Entries cut by the block's end: an 8-byte one, and an F0h whose UID
    // runs past it.
    let mut cut = vec![];
    cut.extend(four(0x02, 1, 0));
    cut.extend([0x42, 2, 0, 0]);
    let ivrs = parse(&table(0, &[block(0x11, &cut)])).unwrap();
    assert_eq!(ivrs.blocks[0].devices, [range(1, 1, 0)]);
    let mut acpi = vec![0xf0, 0, 0, 0];
    acpi.extend([0u8; 16]);
    acpi.extend([1, 200, 1]);
    let ivrs = parse(&table(0, &[block(0x40, &acpi)])).unwrap();
    assert!(ivrs.blocks[0].acpi.is_empty());
}

#[test]
fn a_long_uid_is_cut_to_max_uid() {
    let mut acpi = vec![0xf0, 1, 0, 0];
    acpi.extend([0u8; 16]);
    acpi.extend([2, 60]);
    acpi.extend([b'u'; 60]);
    let mut e = acpi;
    e.extend(four(0x02, 9, 0));
    let ivrs = parse(&table(0, &[block(0x40, &e)])).unwrap();
    let b = &ivrs.blocks[0];
    assert_eq!(usize::from(b.acpi[0].uid_len), MAX_UID);
    assert_eq!(b.devices, [range(9, 9, 0)]);
}

#[test]
fn the_header_is_checked() {
    let good = table(0, &[block(0x10, &[])]);
    assert!(parse(&good).is_ok());
    let mut bad = good.clone();
    bad[..4].copy_from_slice(b"IVRX");
    seal(&mut bad);
    assert_eq!(parse(&bad), Err(IvrsError::Signature));
    let mut bad = good.clone();
    bad[40] ^= 1;
    assert_eq!(parse(&bad), Err(IvrsError::BadChecksum));
    assert_eq!(parse(&good[..good.len() - 1]), Err(IvrsError::Length));
    assert_eq!(parse(&good[..10]), Err(IvrsError::Length));
    assert_eq!(parse(&[]), Err(IvrsError::Length));
}

#[test]
fn iommus_are_kept_apart_by_segment_and_device() {
    let mut other = block(0x11, &[]);
    other[4..6].copy_from_slice(&0x0018u16.to_le_bytes());
    let mut segment = block(0x10, &[]);
    segment[16..18].copy_from_slice(&1u16.to_le_bytes());
    let t = table(0, &[block(0x11, &[]), block(0x10, &[]), other, segment]);
    let ivrs = parse(&t).unwrap();
    let kept: Vec<(u16, u16, Kind)> = ivrs
        .iommus()
        .iter()
        .map(|b| (b.segment, b.device_id, b.kind))
        .collect();
    assert_eq!(
        kept,
        [
            (0, 0x10, Kind::Efr11),
            (0, 0x18, Kind::Efr11),
            (1, 0x10, Kind::Fixed10)
        ]
    );
}
