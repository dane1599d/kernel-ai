//! The controller registers in BAR0 that the driver uses, and what their bits mean.

/// Controller Capabilities, 64 bits.
pub const CAP: usize = 0x00;
/// Version.
pub const VS: usize = 0x08;
/// Interrupt Mask Set: the driver polls, so it masks every vector.
pub const INTMS: usize = 0x0C;
/// Controller Configuration.
pub const CC: usize = 0x14;
/// Controller Status.
pub const CSTS: usize = 0x1C;
/// Admin Queue Attributes.
pub const AQA: usize = 0x24;
/// Admin Submission Queue base address, 64 bits.
pub const ASQ: usize = 0x28;
/// Admin Completion Queue base address, 64 bits.
pub const ACQ: usize = 0x30;
/// Where the doorbells start.
const DOORBELLS: usize = 0x1000;

/// CC.EN.
pub const CC_ENABLE: u32 = 1;
/// CSTS.RDY.
pub const CSTS_READY: u32 = 1;
/// CSTS.CFS: the controller hit a fatal error.
pub const CSTS_FATAL: u32 = 1 << 1;

/// What the driver reads from CAP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Entries a queue may have at most (MQES + 1).
    pub max_entries: u32,
    /// Bytes between doorbells: 4 << DSTRD.
    pub doorbell_stride: usize,
    /// How long CSTS.RDY may take to change, in milliseconds (TO, in 500 ms units).
    pub ready_timeout_ms: u64,
    /// The controller takes the NVM command set (CSS bit 0).
    pub nvm: bool,
    /// The smallest and largest memory page sizes it takes, as powers of two.
    pub page_min_shift: u32,
    pub page_max_shift: u32,
}

impl Capabilities {
    pub fn parse(cap: u64) -> Self {
        Self {
            max_entries: (cap & 0xFFFF) as u32 + 1,
            doorbell_stride: 4 << ((cap >> 32) & 0xF),
            ready_timeout_ms: ((cap >> 24) & 0xFF) * 500,
            nvm: (cap >> 37) & 1 == 1,
            page_min_shift: 12 + ((cap >> 48) & 0xF) as u32,
            page_max_shift: 12 + ((cap >> 52) & 0xF) as u32,
        }
    }

    /// Whether the controller takes the driver's 4 KiB pages.
    pub fn takes_4k_pages(&self) -> bool {
        self.page_min_shift <= 12 && 12 <= self.page_max_shift
    }

    /// Offset of queue `qid`'s submission tail doorbell.
    pub fn sq_tail_doorbell(&self, qid: u16) -> usize {
        DOORBELLS + (2 * usize::from(qid)) * self.doorbell_stride
    }

    /// Offset of queue `qid`'s completion head doorbell.
    pub fn cq_head_doorbell(&self, qid: u16) -> usize {
        DOORBELLS + (2 * usize::from(qid) + 1) * self.doorbell_stride
    }

    /// The bytes of BAR0 the driver touches with `queues` queue pairs, admin
    /// included.
    pub fn bar_bytes(&self, queues: u16) -> usize {
        self.cq_head_doorbell(queues - 1) + 4
    }
}

/// AQA for admin queues of `entries` entries each.
pub fn admin_queue_attributes(entries: u32) -> u32 {
    let n = entries - 1;
    (n << 16) | n
}

/// CC that enables the controller: the NVM command set, 4 KiB pages, round-robin
/// arbitration, 64-byte submission and 16-byte completion entries.
pub fn enable_configuration() -> u32 {
    const IOSQES: u32 = 6 << 16;
    const IOCQES: u32 = 4 << 20;
    IOSQES | IOCQES | CC_ENABLE
}

/// The version as "major.minor.tertiary".
pub fn version(vs: u32) -> (u16, u8, u8) {
    ((vs >> 16) as u16, (vs >> 8) as u8, vs as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_capabilities_parse() {
        // QEMU 8.2's nvme: MQES 2047, CQR, TO 15, DSTRD 0, NVM and admin-only sets,
        // pages 4 KiB to 64 KiB.
        let cap = 0x0040_0020_0f01_07ff_u64;
        let caps = Capabilities::parse(cap);
        assert_eq!(caps.max_entries, 2048);
        assert_eq!(caps.doorbell_stride, 4);
        assert_eq!(caps.ready_timeout_ms, 7500);
        assert!(caps.nvm);
        assert_eq!((caps.page_min_shift, caps.page_max_shift), (12, 16));
        assert!(caps.takes_4k_pages());
    }

    #[test]
    fn doorbells_follow_the_stride() {
        let mut caps = Capabilities::parse(0);
        assert_eq!(caps.sq_tail_doorbell(0), 0x1000);
        assert_eq!(caps.cq_head_doorbell(0), 0x1004);
        assert_eq!(caps.sq_tail_doorbell(1), 0x1008);
        assert_eq!(caps.cq_head_doorbell(1), 0x100C);
        assert_eq!(caps.bar_bytes(2), 0x1010);
        caps = Capabilities::parse(2 << 32);
        assert_eq!(caps.doorbell_stride, 16);
        assert_eq!(caps.cq_head_doorbell(1), 0x1000 + 3 * 16);
    }

    #[test]
    fn configuration_words() {
        assert_eq!(admin_queue_attributes(32), 0x001F_001F);
        assert_eq!(enable_configuration(), 0x0046_0001);
        assert_eq!(version(0x0001_0400), (1, 4, 0));
    }

    #[test]
    fn large_pages_only_is_refused() {
        // MPSMIN = 1: 8 KiB pages at least.
        assert!(!Capabilities::parse(1 << 48).takes_4k_pages());
    }
}
