//! The `acpi_tables` fuzz target's body (plan P6 G34), which
//! `acpitables/tests/hostile.rs` also runs over hostile firmware memory, the
//! way `fuzz/harness.rs` serves its targets: a later
//! `fuzz/fuzz_targets/acpi_tables.rs` includes this file by `#[path]` and
//! calls [`acpi_tables`] with libFuzzer's input. It must return for any input,
//! and only a bug in `acpitables` may panic.
//!
//! The input is firmware memory: a window of physical memory starting at
//! [`BASE`], with the RSDP at its start. A fake [`Window`] refuses every read
//! that is not wholly inside it, and counts reads and refusals. What `parse`
//! must keep: it never asks for more than [`MAX_TABLE_LEN`] at once, it makes
//! a bounded number of reads for the tables the root lists, and a platform it
//! gives holds no more entries than the bytes it read could carry.

#![allow(dead_code)]

use acpitables::{MAX_IGNORED, MAX_TABLE_LEN, PhysMem, Platform, parse};
use std::cell::Cell;

/// Where the window starts: the BIOS area where firmware keeps an RSDP.
pub const BASE: u64 = 0xE_0000;

/// Physical memory `[BASE, BASE + bytes.len())`; every other read refused.
pub struct Window<'a> {
    pub bytes: &'a [u8],
    pub reads: Cell<usize>,
    pub refused: Cell<usize>,
    /// Bytes handed back, summed over the reads.
    pub given: Cell<usize>,
    pub largest: Cell<usize>,
}

impl<'a> Window<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            reads: Cell::new(0),
            refused: Cell::new(0),
            given: Cell::new(0),
            largest: Cell::new(0),
        }
    }
}

impl PhysMem for Window<'_> {
    fn read(&self, phys: u64, len: usize) -> Option<&[u8]> {
        self.reads.set(self.reads.get() + 1);
        self.largest.set(self.largest.get().max(len));
        let inside = phys
            .checked_sub(BASE)
            .and_then(|at| usize::try_from(at).ok())
            .and_then(|at| Some(at..at.checked_add(len)?))
            .and_then(|range| self.bytes.get(range));
        match inside {
            Some(bytes) => {
                self.given.set(self.given.get() + len);
                Some(bytes)
            }
            None => {
                self.refused.set(self.refused.get() + 1);
                None
            }
        }
    }
}

/// Parses `data` as it is, and again with the RSDP's checksums fixed and its
/// root pointer folded into the window, so random bytes reach the root table
/// and the tables it lists, not only the RSDP's checks.
pub fn acpi_tables(data: &[u8]) {
    check(data);
    if data.len() >= 36 {
        let mut fixed = data.to_vec();
        fixed[..8].copy_from_slice(b"RSD PTR ");
        let root = BASE
            + u64::from(u32::from_le_bytes(fixed[24..28].try_into().unwrap())) % data.len() as u64;
        fixed[16..20].copy_from_slice(&(root as u32).to_le_bytes());
        fixed[24..32].copy_from_slice(&root.to_le_bytes());
        fix(&mut fixed[..20], 8);
        fix(&mut fixed[..36], 32);
        check(&fixed);
    }
}

/// Sets `bytes[at]` so `bytes` sums to zero.
pub fn fix(bytes: &mut [u8], at: usize) {
    bytes[at] = 0;
    let sum = bytes.iter().fold(0u8, |s, &b| s.wrapping_add(b));
    bytes[at] = 0u8.wrapping_sub(sum);
}

/// The promises, for the window `data` with the RSDP at [`BASE`].
pub fn check(data: &[u8]) -> Option<Platform> {
    let mem = Window::new(data);
    let result = parse(&mem, BASE);
    assert!(
        mem.largest.get() <= MAX_TABLE_LEN,
        "a read of {}",
        mem.largest.get()
    );
    // The RSDP twice, the root table twice, and for each pointer the root can
    // hold (a 36-byte header, at least 4 bytes each) a signature, a header
    // and the table: no read loops.
    let pointers = MAX_TABLE_LEN.min(data.len()) / 4;
    assert!(
        mem.reads.get() <= 4 + 3 * pointers,
        "{} reads",
        mem.reads.get()
    );
    let platform = result.ok()?;
    // Each entry came from bytes read inside the window: a CPU or an override
    // takes at least 8 bytes of MADT, an I/O APIC 12, an ECAM window 16.
    let given = mem.given.get();
    assert!(8 * platform.cpus.len() <= given);
    assert!(12 * platform.io_apics.len() + 10 * platform.overrides.len() <= given);
    assert!(16 * platform.ecam.len() <= given);
    assert!(platform.ignored.len() <= MAX_IGNORED);
    assert!(platform.reset.is_none_or(|r| r.register.address != 0));
    // An answer that came back is one of the window's.
    assert!(mem.reads.get() > mem.refused.get());
    Some(platform)
}
