//! The body of a later `ivrs` fuzz target (plan P6 G61), which
//! `acpitables/tests/ivrs.rs` runs over hostile tables, the way
//! `harness.rs` serves `acpi_tables`. It must return for any input, and only
//! a bug in `acpitables::ivrs` may panic.
//!
//! What `ivrs::parse` must keep, for any bytes: it never panics; a table it
//! reads holds no more blocks than its bytes could carry (one per 24) and no
//! more entries (one per 4); every range runs forward; a UID is never longer
//! than `MAX_UID`; a block's type is one of the three IVHD types; and it
//! allocates no more than a small multiple of the table.

#![allow(dead_code)]

use acpitables::ivrs::{self, Ivrs, MAX_UID};

/// The most bytes `parse` may allocate per byte of table, on top of a floor.
pub const ALLOC_PER_BYTE: usize = 24;
pub const ALLOC_FLOOR: usize = 4096;

/// Parses `data` as an IVRS table and checks what it gives.
pub fn ivrs_table(data: &[u8]) -> Option<Ivrs> {
    let ivrs = ivrs::parse(data).ok()?;
    let len = data.len();
    assert!(
        ivrs.blocks.len() <= len / 24,
        "{} blocks",
        ivrs.blocks.len()
    );
    let entries: usize = ivrs
        .blocks
        .iter()
        .map(|b| b.devices.len() + b.specials.len() + b.acpi.len())
        .sum();
    assert!(entries <= len / 4, "{entries} entries in {len} bytes");
    for block in &ivrs.blocks {
        for d in &block.devices {
            assert!(d.first <= d.last, "{d:?}");
        }
        for a in &block.acpi {
            assert!(usize::from(a.uid_len) <= MAX_UID, "{a:?}");
        }
        assert!(
            block.acpi.is_empty() || block.kind == ivrs::Kind::Acpi40,
            "ACPI devices in a {:?} block",
            block.kind
        );
        let id = block.devices.first().map_or(0, |d| d.first);
        if let Some(found) = block.device(id) {
            assert!(found.first <= id && id <= found.last);
        }
    }
    assert!(ivrs.iommus().len() <= ivrs.blocks.len());
    // Each thing dropped took at least 4 bytes of the table.
    assert!(
        ivrs.dropped <= len / 4,
        "{} dropped in {len} bytes",
        ivrs.dropped
    );
    Some(ivrs)
}

/// Parses `data` as it is, and again with the signature, length and checksum
/// set, so random bytes reach the blocks, not only the header's checks.
pub fn ivrs_tables(data: &[u8]) {
    ivrs_table(data);
    if data.len() >= 48 {
        let mut sealed = data.to_vec();
        sealed[..4].copy_from_slice(b"IVRS");
        reseal(&mut sealed);
        ivrs_table(&sealed);
    }
}

/// Sets `table`'s length field to its own length and its checksum, so a
/// mutation reaches the blocks instead of stopping at the header.
pub fn reseal(table: &mut [u8]) {
    if table.len() < 10 {
        return;
    }
    let len = table.len() as u32;
    table[4..8].copy_from_slice(&len.to_le_bytes());
    table[9] = 0;
    let sum = table.iter().fold(0u8, |s, &b| s.wrapping_add(b));
    table[9] = sum.wrapping_neg();
}
