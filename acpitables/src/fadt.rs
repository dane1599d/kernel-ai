//! The FADT ("FACP"), for what P1 needs without AML (ADR 0007 section 8): the
//! reset register that reboots the machine, and whether it has an 8042
//! keyboard controller.

use crate::{Platform, u32_at, u64_at};

/// Where a register lives: an ACPI Generic Address Structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gas {
    pub space: Space,
    /// Register width in bits.
    pub width: u8,
    pub address: u64,
}

/// A Generic Address Structure's address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Memory,
    Io,
    /// PCI configuration space of a function on segment 0 bus 0: the address
    /// holds the device (bits 32-47), function (bits 16-31) and offset (0-15).
    PciConfig,
    Other(u8),
}

/// The FADT's reset register and the value written to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reset {
    pub register: Gas,
    pub value: u8,
}

/// Byte offsets in the FADT (ACPI 6.5 table 5.9).
const IAPC_BOOT_ARCH: usize = 109;
const FLAGS: usize = 112;
const RESET_REG: usize = 116;
const RESET_VALUE: usize = 128;
/// IAPC_BOOT_ARCH bit 1: the machine has an 8042 (or an equivalent).
const BOOT_ARCH_8042: u16 = 1 << 1;
/// Flags bit 10: the reset register is supported.
const RESET_REG_SUP: u32 = 1 << 10;

/// Reads `fadt` (a whole, checked table) into `platform`. IAPC_BOOT_ARCH exists
/// from revision 2 on; an older FADT, like none at all, leaves the 8042 assumed.
pub(crate) fn apply(fadt: &[u8], platform: &mut Platform) {
    let revision = fadt.get(8).copied().unwrap_or(0);
    if revision >= 2 && fadt.len() >= IAPC_BOOT_ARCH + 2 {
        let arch = u16::from_le_bytes([fadt[IAPC_BOOT_ARCH], fadt[IAPC_BOOT_ARCH + 1]]);
        platform.no_8042 = arch & BOOT_ARCH_8042 == 0;
    }
    if fadt.len() > RESET_VALUE && u32_at(fadt, FLAGS) & RESET_REG_SUP != 0 {
        let register = Gas {
            space: match fadt[RESET_REG] {
                0 => Space::Memory,
                1 => Space::Io,
                2 => Space::PciConfig,
                other => Space::Other(other),
            },
            width: fadt[RESET_REG + 1],
            address: u64_at(fadt, RESET_REG + 4),
        };
        if register.address != 0 {
            platform.reset = Some(Reset {
                register,
                value: fadt[RESET_VALUE],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// A FADT of `revision` with `arch`, `flags` and the reset register at
    /// `address` in `space`.
    fn fadt(revision: u8, arch: u16, flags: u32, space: u8, address: u64) -> Vec<u8> {
        let mut t = vec![0u8; 276];
        t[..4].copy_from_slice(b"FACP");
        t[8] = revision;
        t[IAPC_BOOT_ARCH..IAPC_BOOT_ARCH + 2].copy_from_slice(&arch.to_le_bytes());
        t[FLAGS..FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        t[RESET_REG] = space;
        t[RESET_REG + 1] = 8;
        t[RESET_REG + 4..RESET_REG + 12].copy_from_slice(&address.to_le_bytes());
        t[RESET_VALUE] = 0x0F;
        t
    }

    #[test]
    fn q35s_reset_register_and_8042_are_read() {
        // QEMU q35: reset by writing 0xF to port 0xCF9; an 8042 is present.
        let mut platform = Platform::default();
        apply(
            &fadt(5, BOOT_ARCH_8042, RESET_REG_SUP, 1, 0xCF9),
            &mut platform,
        );
        let reset = platform.reset.unwrap();
        assert_eq!(reset.register.space, Space::Io);
        assert_eq!(reset.register.address, 0xCF9);
        assert_eq!(reset.register.width, 8);
        assert_eq!(reset.value, 0x0F);
        assert!(!platform.no_8042);
    }

    #[test]
    fn no_8042_is_read_only_from_revision_2() {
        let mut platform = Platform::default();
        apply(&fadt(5, 0, 0, 1, 0xCF9), &mut platform);
        assert!(platform.no_8042);
        assert_eq!(platform.reset, None, "the reset register is not supported");
        let mut platform = Platform::default();
        apply(&fadt(1, 0, 0, 1, 0xCF9), &mut platform);
        assert!(!platform.no_8042, "ACPI 1.0 has no IAPC_BOOT_ARCH");
    }

    #[test]
    fn other_spaces_and_short_tables_are_handled() {
        let mut platform = Platform::default();
        apply(&fadt(5, 0, RESET_REG_SUP, 0, 0xFED0_0000), &mut platform);
        assert_eq!(platform.reset.unwrap().register.space, Space::Memory);
        apply(&fadt(5, 0, RESET_REG_SUP, 9, 0x10), &mut platform);
        assert_eq!(platform.reset.unwrap().register.space, Space::Other(9));
        let mut platform = Platform::default();
        apply(&fadt(5, 0, RESET_REG_SUP, 1, 0), &mut platform);
        assert_eq!(platform.reset, None, "address 0 is no register");
        let mut platform = Platform::default();
        apply(&fadt(5, 0, RESET_REG_SUP, 1, 0xCF9)[..100], &mut platform);
        assert_eq!(
            platform,
            Platform::default(),
            "a short FADT changes nothing"
        );
    }
}
