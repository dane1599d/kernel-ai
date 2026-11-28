//! Which local APIC mode the kernel runs in (P6 G48): xAPIC, memory-mapped,
//! or x2APIC, through MSRs.

/// CPUID 1 ECX: the local APIC has an x2APIC mode.
pub const CPUID_X2APIC: u32 = 1 << 21;

/// Whether CPUID offers x2APIC mode.
#[cfg(target_arch = "x86_64")]
pub fn offered() -> bool {
    #[allow(unused_unsafe)]
    // SAFETY: CPUID leaf 1 exists on every x86_64 CPU and only reads.
    let leaf = unsafe { core::arch::x86_64::__cpuid(1) };
    leaf.ecx & CPUID_X2APIC != 0
}

#[cfg(not(target_arch = "x86_64"))]
pub fn offered() -> bool {
    false
}

/// The mode the boot CPU's local APIC takes, and every other CPU's after it,
/// with the reason, for the boot log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    XApic(&'static str),
    X2Apic(&'static str),
}

/// [`Mode`] from IA32_APIC_BASE as firmware left it (`handed_over`: its x2APIC
/// bit set), CPUID (`offered`), and whether the command line asked
/// (`apic=x2apic`). A local APIC handed over in x2APIC mode stays in it: going
/// back to xAPIC needs it disabled first, which loses its state. Otherwise
/// xAPIC unless asked, so a boot that names nothing is as before.
pub fn mode(handed_over: bool, offered: bool, asked: bool) -> Mode {
    match (handed_over, offered, asked) {
        (true, _, _) => Mode::X2Apic("x2APIC, as firmware handed over"),
        (false, true, true) => Mode::X2Apic("x2APIC (apic=x2apic)"),
        (false, false, true) => Mode::XApic("xAPIC: apic=x2apic, but CPUID offers no x2APIC"),
        (false, _, false) => Mode::XApic("xAPIC"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x2apic_is_taken_when_handed_over_or_asked_and_offered() {
        for offered in [false, true] {
            for asked in [false, true] {
                assert!(matches!(mode(true, offered, asked), Mode::X2Apic(_)));
            }
        }
        assert_eq!(
            mode(false, true, true),
            Mode::X2Apic("x2APIC (apic=x2apic)")
        );
        assert!(matches!(mode(false, false, true), Mode::XApic(why) if why.contains("no x2APIC")));
        assert_eq!(mode(false, true, false), Mode::XApic("xAPIC"));
        assert_eq!(mode(false, false, false), Mode::XApic("xAPIC"));
    }
}
