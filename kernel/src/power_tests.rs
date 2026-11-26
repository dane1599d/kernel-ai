//! `test=reboot-memory` (bug hunt 3, H3-2): a memory-space reset register is
//! written with this CPU holding the memory lock and interrupts off, as a
//! reboot from a fault inside `memory::with` does.

use crate::{log, memory, power};
use acpitables::fadt::{Gas, Reset, Space};

pub fn check() -> Result<(), &'static str> {
    // A RAM byte stands in for the register. Mapped for good, so never freed.
    // The uncached alias of a frame the HHDM maps write-back is a memory-type
    // conflict, harmless under QEMU (TCG, and KVM forces write-back on RAM) but
    // never to be run on bare metal: this case is for QEMU only.
    let frame = memory::with(|m| m.allocate_zeroed_frame()).ok_or("power: no frame")?;
    let reset = Reset {
        register: Gas {
            space: Space::Memory,
            width: 8,
            address: frame + 0x10,
        },
        value: 0x5A,
    };
    power::map_reset(reset)?;
    // Mapping here would spin on the held lock (or trip `with`'s assertion).
    memory::with(|_| {
        x86_64::instructions::interrupts::without_interrupts(|| power::write_reset(reset));
    });
    // SAFETY: the frame is this test's and the HHDM maps it.
    let byte = unsafe { ((memory::hhdm() + frame + 0x10) as *const u8).read_volatile() };
    if byte != 0x5A {
        return Err("power: the memory reset register was not written");
    }
    log!("power: a memory reset register was written with the memory lock held");
    Ok(())
}
