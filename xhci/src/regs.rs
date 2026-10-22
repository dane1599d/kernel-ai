//! The registers in BAR0: the capability registers at its start, the
//! operational registers after them, the runtime registers with the
//! interrupters, and the doorbell array. Offsets here are from BAR0 unless a
//! constant says otherwise.

/// CAPLENGTH (low byte) and HCIVERSION (high half).
pub const CAPLENGTH: usize = 0x00;
pub const HCSPARAMS1: usize = 0x04;
pub const HCSPARAMS2: usize = 0x08;
pub const HCCPARAMS1: usize = 0x10;
pub const DBOFF: usize = 0x14;
pub const RTSOFF: usize = 0x18;

/// Operational registers, from the operational base.
pub const USBCMD: usize = 0x00;
pub const USBSTS: usize = 0x04;
pub const PAGESIZE: usize = 0x08;
pub const CRCR: usize = 0x18;
pub const DCBAAP: usize = 0x30;
pub const CONFIG: usize = 0x38;
/// The first port's register set; each port has 16 bytes.
const PORTS: usize = 0x400;

pub const CMD_RUN: u32 = 1;
pub const CMD_RESET: u32 = 1 << 1;
pub const CMD_INTERRUPTS: u32 = 1 << 2;

pub const STS_HALTED: u32 = 1;
pub const STS_HOST_ERROR: u32 = 1 << 2;
pub const STS_EVENT: u32 = 1 << 3;
pub const STS_NOT_READY: u32 = 1 << 11;
pub const STS_CONTROLLER_ERROR: u32 = 1 << 12;

/// CRCR.RCS: the command ring's first cycle state.
pub const CRCR_CYCLE: u64 = 1;

/// Interrupter 0's registers, from the runtime base.
pub const IMAN: usize = 0x20;
pub const IMOD: usize = 0x24;
pub const ERSTSZ: usize = 0x28;
pub const ERSTBA: usize = 0x30;
pub const ERDP: usize = 0x38;

/// IMAN.IP, written 1 to clear, and IMAN.IE.
pub const IMAN_PENDING: u32 = 1;
pub const IMAN_ENABLE: u32 = 1 << 1;
/// ERDP.EHB: written 1 to clear once the handler has caught up.
pub const ERDP_BUSY: u64 = 1 << 3;

/// What the driver reads from the capability registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Where the operational registers start.
    pub operational: usize,
    pub version: u16,
    pub max_slots: u8,
    pub max_ports: u8,
    /// Scratchpad pages the controller needs from the driver.
    pub scratchpads: u16,
    /// The controller reads and writes 64-bit addresses (AC64).
    pub addr64: bool,
    /// Contexts are 64 bytes (CSZ), not 32.
    pub context_bytes: usize,
    /// Ports have power switches (PPC), so the driver must power them.
    pub port_power: bool,
    /// Where the extended capabilities start, if any.
    pub extended: Option<usize>,
    pub doorbells: usize,
    pub runtime: usize,
}

impl Capabilities {
    /// Parses the registers `read` returns for each capability offset.
    pub fn parse(read: impl Fn(usize) -> u32) -> Self {
        let caplength = read(CAPLENGTH);
        let params1 = read(HCSPARAMS1);
        let params2 = read(HCSPARAMS2);
        let cparams1 = read(HCCPARAMS1);
        let extended = ((cparams1 >> 16) as usize) * 4;
        Self {
            operational: (caplength & 0xFF) as usize,
            version: (caplength >> 16) as u16,
            max_slots: params1 as u8,
            max_ports: (params1 >> 24) as u8,
            scratchpads: (((params2 >> 21) & 0x1F) << 5 | (params2 >> 27)) as u16,
            addr64: cparams1 & 1 != 0,
            context_bytes: if cparams1 & (1 << 2) != 0 { 64 } else { 32 },
            port_power: cparams1 & (1 << 3) != 0,
            extended: (extended != 0).then_some(extended),
            doorbells: (read(DBOFF) & !0x3) as usize,
            runtime: (read(RTSOFF) & !0x1F) as usize,
        }
    }

    /// BAR0 offset of operational register `reg`.
    pub fn op(&self, reg: usize) -> usize {
        self.operational + reg
    }

    /// BAR0 offset of runtime register `reg`.
    pub fn rt(&self, reg: usize) -> usize {
        self.runtime + reg
    }

    /// BAR0 offset of PORTSC for 1-based `port`.
    pub fn portsc(&self, port: u8) -> usize {
        self.operational + PORTS + 16 * (usize::from(port) - 1)
    }

    /// BAR0 offset of doorbell `slot` (0 rings the command ring).
    pub fn doorbell(&self, slot: u8) -> usize {
        self.doorbells + 4 * usize::from(slot)
    }

    /// The last register offset the driver touches, so the caller can check its
    /// mapping covers it.
    pub fn span(&self) -> usize {
        let ports = self.portsc(self.max_ports.max(1)) + 16;
        let doorbells = self.doorbell(self.max_slots) + 4;
        let runtime = self.rt(ERDP) + 8;
        ports.max(doorbells).max(runtime)
    }
}

/// PORTSC bits.
pub mod port {
    pub const CONNECTED: u32 = 1;
    /// PED: writing 1 disables the port, so writes keep it 0.
    pub const ENABLED: u32 = 1 << 1;
    pub const RESET: u32 = 1 << 4;
    pub const POWER: u32 = 1 << 9;
    /// The change bits, each written 1 to clear.
    pub const CHANGES: u32 = 0x7F << 17;
    pub const RESET_CHANGE: u32 = 1 << 21;
    /// The bits a write must carry over unchanged: power, indicator and wake.
    const PRESERVE: u32 = POWER | (0x3 << 14) | (0x7 << 25);

    /// PORTSC's speed field: 1 full, 2 low, 3 high, 4 and up super speed.
    pub fn speed(portsc: u32) -> u8 {
        ((portsc >> 10) & 0xF) as u8
    }

    /// The value to write to change only the bits in `set`: change bits other
    /// than those in `set` stay uncleared and PED stays enabled.
    pub fn write_value(portsc: u32, set: u32) -> u32 {
        (portsc & PRESERVE) | set
    }
}

/// Speed numbers from PORTSC and in slot contexts.
pub mod speed {
    pub const FULL: u8 = 1;
    pub const LOW: u8 = 2;
    pub const HIGH: u8 = 3;
    pub const SUPER: u8 = 4;

    /// The control endpoint's packet size to start with, before the device
    /// descriptor says: the smallest a device of that speed may have.
    pub fn default_packet_size(speed: u8) -> u16 {
        match speed {
            LOW | FULL => 8,
            HIGH => 64,
            _ => 512,
        }
    }

    pub fn name(speed: u8) -> &'static str {
        match speed {
            FULL => "full speed",
            LOW => "low speed",
            HIGH => "high speed",
            SUPER => "super speed",
            _ => "super speed plus",
        }
    }
}

/// PAGESIZE bit 0: the controller takes 4 KiB pages.
pub fn takes_4k_pages(pagesize: u32) -> bool {
    pagesize & 1 != 0
}

/// The USB legacy support capability (ID 1), through which firmware hands the
/// controller to the OS.
pub mod legacy {
    pub const ID: u8 = 1;
    /// HC BIOS Owned and HC OS Owned semaphores, in the capability's first dword.
    pub const BIOS_OWNED: u32 = 1 << 16;
    pub const OS_OWNED: u32 = 1 << 24;
    /// The control and status dword after it: SMI enables in the low half.
    pub const CONTROL: usize = 4;
    pub const SMI_ENABLES: u32 = 0xE01F;
}

/// Walks the extended capability list from `first`, returning the offset of the
/// first capability with `id`. `read` reads a dword at a BAR0 offset; the walk
/// stops after 64 entries in case a list loops.
pub fn find_extended(first: usize, id: u8, read: impl Fn(usize) -> u32) -> Option<usize> {
    let mut at = first;
    for _ in 0..64 {
        let header = read(at);
        if header as u8 == id {
            return Some(at);
        }
        let next = ((header >> 8) & 0xFF) as usize * 4;
        if next == 0 {
            return None;
        }
        at += next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QEMU's qemu-xhci capability registers.
    fn qemu(offset: usize) -> u32 {
        match offset {
            CAPLENGTH => 0x0100_0040,
            HCSPARAMS1 => 0x0800_0740,
            HCSPARAMS2 => 0x0000_000F,
            HCCPARAMS1 => 0x0008_0001,
            DBOFF => 0x2000,
            RTSOFF => 0x1000,
            _ => 0,
        }
    }

    #[test]
    fn capabilities_parse_like_qemus_controller() {
        let caps = Capabilities::parse(qemu);
        assert_eq!(caps.operational, 0x40);
        assert_eq!(caps.version, 0x100);
        assert_eq!(caps.max_slots, 64);
        assert_eq!(caps.max_ports, 8);
        assert_eq!(caps.scratchpads, 0);
        assert!(caps.addr64);
        assert_eq!(caps.context_bytes, 32);
        assert!(!caps.port_power);
        assert_eq!(caps.extended, Some(0x20));
        assert_eq!(caps.op(USBSTS), 0x44);
        assert_eq!(caps.rt(ERDP), 0x1038);
        assert_eq!(caps.portsc(1), 0x440);
        assert_eq!(caps.portsc(8), 0x4B0);
        assert_eq!(caps.doorbell(3), 0x200C);
        assert_eq!(caps.span(), 0x2000 + 4 * 64 + 4);
    }

    #[test]
    fn scratchpad_count_joins_its_two_fields() {
        let caps = Capabilities::parse(|offset| match offset {
            // Hi = 1, Lo = 3: 32 + 3 pages.
            HCSPARAMS2 => (1 << 21) | (3 << 27),
            HCCPARAMS1 => 1 << 2,
            _ => 0,
        });
        assert_eq!(caps.scratchpads, 35);
        assert_eq!(caps.context_bytes, 64);
        assert_eq!(caps.extended, None);
    }

    #[test]
    fn port_writes_keep_power_and_never_clear_or_disable_by_accident() {
        // Connected, enabled, powered, high speed, with connect and reset changes.
        let portsc = port::CONNECTED
            | port::ENABLED
            | port::POWER
            | (3 << 10)
            | (1 << 17)
            | port::RESET_CHANGE;
        assert_eq!(port::speed(portsc), speed::HIGH);
        assert_eq!(
            port::write_value(portsc, port::RESET),
            port::POWER | port::RESET
        );
        assert_eq!(
            port::write_value(portsc, port::RESET_CHANGE),
            port::POWER | port::RESET_CHANGE
        );
    }

    #[test]
    fn default_packet_sizes_follow_the_speed() {
        assert_eq!(speed::default_packet_size(speed::LOW), 8);
        assert_eq!(speed::default_packet_size(speed::FULL), 8);
        assert_eq!(speed::default_packet_size(speed::HIGH), 64);
        assert_eq!(speed::default_packet_size(speed::SUPER), 512);
    }

    #[test]
    fn the_extended_list_is_walked_to_the_wanted_id() {
        // Supported protocol (2) at 0x20 with next 4 dwords, legacy (1) at 0x30.
        let read = |at| match at {
            0x20 => 0x0200_0402,
            0x30 => 0x0001_0001,
            _ => 0,
        };
        assert_eq!(find_extended(0x20, legacy::ID, read), Some(0x30));
        assert_eq!(find_extended(0x20, 10, read), None);
        // A list that never ends stops after 64 entries.
        assert_eq!(find_extended(0x20, 9, |_| 0x0000_0102), None);
    }
}
