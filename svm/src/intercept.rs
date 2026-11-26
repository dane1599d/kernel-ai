//! What exits to the hypervisor (APM vol. 2, 15.7-15.13 and table B-1): the
//! six intercept vectors at the VMCB's start, the 8 KiB MSR permission map
//! and the 12 KiB I/O permission map.

use crate::vmcb::ControlArea;

/// One intercept bit: `word * 32 + bit` over the control area's six 32-bit
/// intercept vectors (VMCB 0x000-0x017).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Intercept(pub u16);

impl Intercept {
    /// A read of CR`n` (word 0, bits 15:0).
    pub const fn cr_read(n: u8) -> Self {
        Self((n & 0xF) as u16)
    }
    /// A write of CR`n` (word 0, bits 31:16).
    pub const fn cr_write(n: u8) -> Self {
        Self(16 + (n & 0xF) as u16)
    }
    /// A read of DR`n` (word 1, bits 15:0).
    pub const fn dr_read(n: u8) -> Self {
        Self(32 + (n & 0xF) as u16)
    }
    /// A write of DR`n` (word 1, bits 31:16).
    pub const fn dr_write(n: u8) -> Self {
        Self(48 + (n & 0xF) as u16)
    }
    /// Exception `vector` (word 2).
    pub const fn exception(vector: u8) -> Self {
        Self(64 + (vector & 0x1F) as u16)
    }

    // Word 3 (VMCB 0x00C).
    pub const INTR: Self = Self(96);
    pub const NMI: Self = Self(97);
    pub const SMI: Self = Self(98);
    pub const INIT: Self = Self(99);
    pub const VINTR: Self = Self(100);
    pub const CR0_SELECTIVE_WRITE: Self = Self(101);
    pub const IDTR_READ: Self = Self(102);
    pub const GDTR_READ: Self = Self(103);
    pub const LDTR_READ: Self = Self(104);
    pub const TR_READ: Self = Self(105);
    pub const IDTR_WRITE: Self = Self(106);
    pub const GDTR_WRITE: Self = Self(107);
    pub const LDTR_WRITE: Self = Self(108);
    pub const TR_WRITE: Self = Self(109);
    pub const RDTSC: Self = Self(110);
    pub const RDPMC: Self = Self(111);
    pub const PUSHF: Self = Self(112);
    pub const POPF: Self = Self(113);
    pub const CPUID: Self = Self(114);
    pub const RSM: Self = Self(115);
    pub const IRET: Self = Self(116);
    pub const INTN: Self = Self(117);
    pub const INVD: Self = Self(118);
    pub const PAUSE: Self = Self(119);
    pub const HLT: Self = Self(120);
    pub const INVLPG: Self = Self(121);
    pub const INVLPGA: Self = Self(122);
    /// Port I/O, as the IOPM selects ([`IoPermissions`]).
    pub const IOIO_PROT: Self = Self(123);
    /// RDMSR and WRMSR, as the MSRPM selects ([`MsrPermissions`]).
    pub const MSR_PROT: Self = Self(124);
    pub const TASK_SWITCH: Self = Self(125);
    pub const FERR_FREEZE: Self = Self(126);
    /// A shutdown (triple fault); without it the CPU shuts down, host and all.
    pub const SHUTDOWN: Self = Self(127);

    // Word 4 (VMCB 0x010).
    /// VMRUN in the guest. VMRUN refuses a VMCB without it ([`Intercepts::required`]).
    pub const VMRUN: Self = Self(128);
    pub const VMMCALL: Self = Self(129);
    pub const VMLOAD: Self = Self(130);
    pub const VMSAVE: Self = Self(131);
    pub const STGI: Self = Self(132);
    pub const CLGI: Self = Self(133);
    pub const SKINIT: Self = Self(134);
    pub const RDTSCP: Self = Self(135);
    pub const ICEBP: Self = Self(136);
    pub const WBINVD: Self = Self(137);
    pub const MONITOR: Self = Self(138);
    pub const MWAIT: Self = Self(139);
    pub const MWAIT_CONDITIONAL: Self = Self(140);
    pub const XSETBV: Self = Self(141);
    pub const RDPRU: Self = Self(142);
    pub const EFER_WRITE_TRAP: Self = Self(143);
    /// A trap after a write of CR`n` (word 4, bits 31:16).
    pub const fn cr_write_trap(n: u8) -> Self {
        Self(144 + (n & 0xF) as u16)
    }

    // Word 5 (VMCB 0x014).
    pub const INVLPGB: Self = Self(160);
    pub const INVLPGB_ILLEGAL: Self = Self(161);
    pub const INVPCID: Self = Self(162);
    pub const MCOMMIT: Self = Self(163);
    pub const TLBSYNC: Self = Self(164);
    pub const BUS_LOCK: Self = Self(165);
    pub const IDLE_HLT: Self = Self(166);

    const fn word(self) -> usize {
        (self.0 / 32) as usize
    }

    const fn bit(self) -> u32 {
        1 << (self.0 % 32)
    }
}

/// A set of intercepts: the six vectors as the VMCB holds them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Intercepts(pub [u32; 6]);

impl Intercepts {
    pub const fn none() -> Self {
        Self([0; 6])
    }

    /// What every VMCB must intercept: VMRUN (APM 15.5.1; VMRUN refuses a
    /// VMCB without it, with [`crate::ExitCode::INVALID`]), and a shutdown,
    /// so a guest's triple fault exits to the host instead of resetting it.
    pub const fn required() -> Self {
        Self::none()
            .with(Intercept::VMRUN)
            .with(Intercept::SHUTDOWN)
    }

    /// The set with `intercept` added.
    pub const fn with(mut self, intercept: Intercept) -> Self {
        self.0[intercept.word()] |= intercept.bit();
        self
    }

    /// The set with `intercept` removed.
    pub const fn without(mut self, intercept: Intercept) -> Self {
        self.0[intercept.word()] &= !intercept.bit();
        self
    }

    pub const fn contains(&self, intercept: Intercept) -> bool {
        self.0[intercept.word()] & intercept.bit() != 0
    }

    /// Writes the set into `control`'s intercept vectors. The caller clears
    /// [`crate::vmcb::Clean::Intercepts`] before the next VMRUN.
    pub fn apply(&self, control: &mut ControlArea) {
        control.intercepts = self.0;
    }

    /// The set `control` holds.
    pub const fn of(control: &ControlArea) -> Self {
        Self(control.intercepts)
    }
}

/// The MSR permission map's size: 8 KiB, physically contiguous.
pub const MSRPM_SIZE: usize = 8192;
/// The I/O permission map's size: 12 KiB, physically contiguous. A port
/// access's every byte is checked, so the last page covers accesses that run
/// past port 0xFFFF.
pub const IOPM_SIZE: usize = 12288;

/// Where MSR `msr`'s two bits are in the MSRPM (APM 15.11): its byte, and
/// the read bit's place in it (the write bit is the next one). None for an
/// MSR outside the map's three ranges, which is always intercepted.
pub const fn msr_bit(msr: u32) -> Option<(usize, u8)> {
    let (base, from) = match msr {
        0x0000_0000..=0x0000_1FFF => (0x0000, 0x0000_0000),
        0xC000_0000..=0xC000_1FFF => (0x0800, 0xC000_0000),
        0xC001_0000..=0xC001_1FFF => (0x1000, 0xC001_0000),
        _ => return None,
    };
    let bit = (msr - from) as usize * 2;
    Some((base + bit / 8, (bit % 8) as u8))
}

/// The MSR permission map: two bits an MSR, read then write; a set bit
/// intercepts. It starts intercepting everything, and the kernel's
/// allow-list (ADR 0006 item 2) opens what the guest may touch itself.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct MsrPermissions(pub [u8; MSRPM_SIZE]);

impl MsrPermissions {
    /// Every MSR intercepted, read and write.
    pub const fn intercept_all() -> Self {
        Self([0xFF; MSRPM_SIZE])
    }

    /// Lets the guest read and/or write `msr` without an exit; the other
    /// direction is left as it was. Refused for an MSR outside the map.
    pub fn allow(&mut self, msr: u32, read: bool, write: bool) -> Result<(), u32> {
        let (byte, bit) = msr_bit(msr).ok_or(msr)?;
        if read {
            self.0[byte] &= !(1 << bit);
        }
        if write {
            self.0[byte] &= !(1 << (bit + 1));
        }
        Ok(())
    }

    /// Whether a read of `msr`, and a write, exit.
    pub const fn intercepts(&self, msr: u32) -> (bool, bool) {
        match msr_bit(msr) {
            Some((byte, bit)) => (
                self.0[byte] & (1 << bit) != 0,
                self.0[byte] & (1 << (bit + 1)) != 0,
            ),
            None => (true, true),
        }
    }
}

/// The I/O permission map: one bit a port; a set bit intercepts.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct IoPermissions(pub [u8; IOPM_SIZE]);

impl IoPermissions {
    /// Every port intercepted, and the bits past 0xFFFF set as well.
    pub const fn intercept_all() -> Self {
        Self([0xFF; IOPM_SIZE])
    }

    /// Lets the guest use ports `first` to `first + count - 1` without an
    /// exit.
    pub fn allow(&mut self, first: u16, count: u16) {
        for port in first as usize..first as usize + count as usize {
            self.0[port / 8] &= !(1 << (port % 8));
        }
    }

    /// Whether an access of `size` bytes at `port` exits: any of its bytes'
    /// ports intercepted, as the CPU checks them.
    pub const fn intercepts(&self, port: u16, size: u8) -> bool {
        let mut at = port as usize;
        while at < port as usize + size as usize {
            if self.0[at / 8] & (1 << (at % 8)) != 0 {
                return true;
            }
            at += 1;
        }
        false
    }
}
