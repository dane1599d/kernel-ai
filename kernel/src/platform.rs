//! Firmware tables: reads the MADT and HPET table through the device window.

use crate::memory;
use acpitables::{PhysMem, Platform};
use spin::Once;
use x86_64::structures::paging::PageTableFlags;

static PLATFORM: Once<Platform> = Once::new();

struct KernelPhys;

impl PhysMem for KernelPhys {
    fn read(&self, phys: u64, len: usize) -> Option<&[u8]> {
        let flags = PageTableFlags::PRESENT | PageTableFlags::NO_EXECUTE;
        let addr = memory::map_physical(phys, len as u64, flags).ok()?;
        // SAFETY: the range was just mapped read-only and mappings are never removed.
        Some(unsafe { core::slice::from_raw_parts(addr as *const u8, len) })
    }
}

/// Parses the ACPI tables. `rsdp` is what Limine reported, which may be a physical
/// address or an HHDM address depending on the base revision.
pub fn init(rsdp: u64, hhdm: u64) -> Result<&'static Platform, &'static str> {
    let phys = if rsdp >= hhdm { rsdp - hhdm } else { rsdp };
    let platform = acpitables::parse(&KernelPhys, phys).map_err(|error| match error {
        acpitables::Error::Unreadable => "ACPI: table memory unreadable",
        acpitables::Error::BadRsdp => "ACPI: RSDP signature missing",
        acpitables::Error::BadChecksum(_) => "ACPI: table checksum mismatch",
        acpitables::Error::NoMadt => "ACPI: no MADT, cannot find the APICs",
    })?;
    Ok(PLATFORM.call_once(|| platform))
}
