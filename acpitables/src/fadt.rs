//! The FADT ("FACP"), for what P1 needs without AML (ADR 0007 section 8): the
//! reset register that reboots the machine, whether it has an 8042 keyboard
//! controller, and the ACPI PM timer, the reference clock on a machine with no
//! HPET (P6 G57).

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

/// The ACPI PM timer: a free-running counter at 3.579545 MHz in I/O space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PmTimer {
    pub port: u16,
    /// TMR_VAL_EXT: the counter is 32 bits wide, not 24.
    pub bits32: bool,
}

/// The PM timer's rate, fixed by the ACPI spec.
pub const PM_TIMER_HZ: u64 = 3_579_545;

/// Byte offsets in the FADT (ACPI 6.5 table 5.9).
const PM_TMR_BLK: usize = 76;
const PM_TMR_LEN: usize = 91;
const X_PM_TMR_BLK: usize = 208;
const IAPC_BOOT_ARCH: usize = 109;
const FLAGS: usize = 112;
const RESET_REG: usize = 116;
const RESET_VALUE: usize = 128;
/// IAPC_BOOT_ARCH bit 1: the machine has an 8042 (or an equivalent).
const BOOT_ARCH_8042: u16 = 1 << 1;
/// Flags bit 10: the reset register is supported.
const RESET_REG_SUP: u32 = 1 << 10;
/// Flags bit 8: the PM timer's counter is 32 bits.
const TMR_VAL_EXT: u32 = 1 << 8;

/// Reads `fadt` (a whole, checked table) into `platform`. IAPC_BOOT_ARCH exists
/// from revision 2 on; an older FADT, like none at all, leaves the 8042 assumed.
pub(crate) fn apply(fadt: &[u8], platform: &mut Platform) {
    let revision = fadt.get(8).copied().unwrap_or(0);
    if revision >= 2 && fadt.len() >= IAPC_BOOT_ARCH + 2 {
        let arch = u16::from_le_bytes([fadt[IAPC_BOOT_ARCH], fadt[IAPC_BOOT_ARCH + 1]]);
        platform.no_8042 = arch & BOOT_ARCH_8042 == 0;
    }
    platform.pm_timer = pm_timer(fadt);
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

/// The PM timer's port: X_PM_TMR_BLK when it is an I/O address, else
/// PM_TMR_BLK with its length of 4. A timer only in memory space is not read
/// here, and none at all is a hardware-reduced machine's.
fn pm_timer(fadt: &[u8]) -> Option<PmTimer> {
    if fadt.len() < FLAGS + 4 {
        return None;
    }
    let extended = (fadt.len() >= X_PM_TMR_BLK + 12)
        .then(|| (fadt[X_PM_TMR_BLK], u64_at(fadt, X_PM_TMR_BLK + 4)))
        .filter(|&(_, address)| address != 0);
    let address = match extended {
        Some((1, address)) => address,
        _ if fadt[PM_TMR_LEN] == 4 => u64::from(u32_at(fadt, PM_TMR_BLK)),
        _ => return None,
    };
    let port = u16::try_from(address).ok().filter(|&port| port != 0)?;
    Some(PmTimer {
        port,
        bits32: u32_at(fadt, FLAGS) & TMR_VAL_EXT != 0,
    })
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
    fn the_pm_timer_is_read_from_x_pm_tmr_blk_or_pm_tmr_blk() {
        // QEMU q35: port 0x608, 24 bits.
        let mut t = fadt(5, BOOT_ARCH_8042, RESET_REG_SUP, 1, 0xCF9);
        t[PM_TMR_BLK..PM_TMR_BLK + 4].copy_from_slice(&0x608u32.to_le_bytes());
        t[PM_TMR_LEN] = 4;
        let mut platform = Platform::default();
        apply(&t, &mut platform);
        let legacy = PmTimer {
            port: 0x608,
            bits32: false,
        };
        assert_eq!(platform.pm_timer, Some(legacy));
        // X_PM_TMR_BLK in I/O space wins, and TMR_VAL_EXT makes it 32 bits.
        t[X_PM_TMR_BLK] = 1;
        t[X_PM_TMR_BLK + 4..X_PM_TMR_BLK + 12].copy_from_slice(&0x1808u64.to_le_bytes());
        t[FLAGS..FLAGS + 4].copy_from_slice(&(RESET_REG_SUP | TMR_VAL_EXT).to_le_bytes());
        apply(&t, &mut platform);
        let extended = PmTimer {
            port: 0x1808,
            bits32: true,
        };
        assert_eq!(platform.pm_timer, Some(extended));
        // In memory space it is not read; PM_TMR_BLK stands in.
        t[X_PM_TMR_BLK] = 0;
        apply(&t, &mut platform);
        assert_eq!(platform.pm_timer.map(|pm| pm.port), Some(0x608));
        // A wrong length, a port past 16 bits, and none at all.
        t[PM_TMR_LEN] = 2;
        apply(&t, &mut platform);
        assert_eq!(platform.pm_timer, None);
        t[X_PM_TMR_BLK] = 1;
        t[X_PM_TMR_BLK + 4..X_PM_TMR_BLK + 12].copy_from_slice(&0x1_0000u64.to_le_bytes());
        apply(&t, &mut platform);
        assert_eq!(platform.pm_timer, None);
        apply(&fadt(5, 0, 0, 1, 0xCF9), &mut platform);
        assert_eq!(platform.pm_timer, None);
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
