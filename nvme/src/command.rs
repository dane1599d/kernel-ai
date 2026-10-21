//! Submission queue entries (64 bytes) and completion queue entries (16 bytes).

/// Admin opcodes.
const CREATE_IO_SQ: u8 = 0x01;
const CREATE_IO_CQ: u8 = 0x05;
const IDENTIFY: u8 = 0x06;
/// NVM command set opcode.
const READ: u8 = 0x02;

/// Identify's CNS values the driver uses.
pub const CNS_NAMESPACE: u32 = 0x00;
pub const CNS_CONTROLLER: u32 = 0x01;
pub const CNS_ACTIVE_NAMESPACES: u32 = 0x02;

/// One submission queue entry, as the sixteen dwords the controller reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Command {
    pub dwords: [u32; 16],
}

impl Command {
    fn new(opcode: u8, cid: u16, nsid: u32, prp1: u64, prp2: u64) -> Self {
        let mut dwords = [0; 16];
        // CDW0: opcode, FUSE 0, PSDT 0 (PRPs), command identifier.
        dwords[0] = u32::from(opcode) | (u32::from(cid) << 16);
        dwords[1] = nsid;
        dwords[6] = prp1 as u32;
        dwords[7] = (prp1 >> 32) as u32;
        dwords[8] = prp2 as u32;
        dwords[9] = (prp2 >> 32) as u32;
        Self { dwords }
    }

    /// Identify into the 4 KiB page at `page`.
    pub fn identify(cid: u16, cns: u32, nsid: u32, page: u64) -> Self {
        let mut command = Self::new(IDENTIFY, cid, nsid, page, 0);
        command.dwords[10] = cns;
        command
    }

    /// Creates I/O completion queue `qid` of `entries` entries in the contiguous
    /// memory at `base`, with interrupts off: the driver polls.
    pub fn create_io_cq(cid: u16, qid: u16, entries: u32, base: u64) -> Self {
        let mut command = Self::new(CREATE_IO_CQ, cid, 0, base, 0);
        command.dwords[10] = ((entries - 1) << 16) | u32::from(qid);
        // PC = 1 (physically contiguous), IEN = 0.
        command.dwords[11] = 1;
        command
    }

    /// Creates I/O submission queue `qid`, completing on queue `cqid`.
    pub fn create_io_sq(cid: u16, qid: u16, entries: u32, base: u64, cqid: u16) -> Self {
        let mut command = Self::new(CREATE_IO_SQ, cid, 0, base, 0);
        command.dwords[10] = ((entries - 1) << 16) | u32::from(qid);
        // CQID, priority 0, PC = 1.
        command.dwords[11] = (u32::from(cqid) << 16) | 1;
        command
    }

    /// Reads `blocks` logical blocks (1 to 65,536) from `lba` of namespace `nsid`.
    pub fn read(cid: u16, nsid: u32, lba: u64, blocks: u32, prp1: u64, prp2: u64) -> Self {
        let mut command = Self::new(READ, cid, nsid, prp1, prp2);
        command.dwords[10] = lba as u32;
        command.dwords[11] = (lba >> 32) as u32;
        // NLB is zero-based.
        command.dwords[12] = (blocks - 1) & 0xFFFF;
        command
    }

    pub fn cid(&self) -> u16 {
        (self.dwords[0] >> 16) as u16
    }
}

/// One completion queue entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Where the controller's submission head was.
    pub sq_head: u16,
    pub sq_id: u16,
    pub cid: u16,
    /// The phase tag: flips each time the controller wraps the queue.
    pub phase: bool,
    /// Status Code Type and Status Code; 0 is success.
    pub status: u16,
}

impl Completion {
    pub fn parse(dwords: [u32; 4]) -> Self {
        let dw3 = dwords[3];
        Self {
            sq_head: dwords[2] as u16,
            sq_id: (dwords[2] >> 16) as u16,
            cid: dw3 as u16,
            phase: (dw3 >> 16) & 1 == 1,
            // Bits 17..28: SC (8 bits) then SCT (3 bits).
            status: ((dw3 >> 17) & 0x7FF) as u16,
        }
    }

    pub fn ok(&self) -> bool {
        self.status == 0
    }

    /// The status as "type/code", for a log line.
    pub fn status_parts(&self) -> (u16, u16) {
        (self.status >> 8, self.status & 0xFF)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_puts_cns_and_page_where_the_spec_says() {
        let command = Command::identify(7, CNS_CONTROLLER, 0, 0x1_2345_6000);
        assert_eq!(command.dwords[0], 0x0007_0006);
        assert_eq!(command.cid(), 7);
        assert_eq!(command.dwords[6], 0x2345_6000);
        assert_eq!(command.dwords[7], 0x1);
        assert_eq!(command.dwords[10], 1);
    }

    #[test]
    fn queue_creation_words() {
        let cq = Command::create_io_cq(1, 1, 64, 0x8000);
        assert_eq!(cq.dwords[0] & 0xFF, 0x05);
        assert_eq!(cq.dwords[10], (63 << 16) | 1);
        assert_eq!(cq.dwords[11], 1);
        let sq = Command::create_io_sq(2, 1, 64, 0x9000, 1);
        assert_eq!(sq.dwords[0] & 0xFF, 0x01);
        assert_eq!(sq.dwords[10], (63 << 16) | 1);
        assert_eq!(sq.dwords[11], (1 << 16) | 1);
        assert_eq!(sq.dwords[6], 0x9000);
    }

    #[test]
    fn read_has_a_zero_based_block_count() {
        let read = Command::read(9, 1, 0x1_0000_0002, 1024, 0xA000, 0xB000);
        assert_eq!(read.dwords[0], 0x0009_0002);
        assert_eq!(read.dwords[1], 1);
        assert_eq!(read.dwords[10], 2);
        assert_eq!(read.dwords[11], 1);
        assert_eq!(read.dwords[12], 1023);
        assert_eq!(read.dwords[8], 0xB000);
    }

    #[test]
    fn completions_parse_phase_and_status() {
        let done = Completion::parse([0, 0, 0x0001_0005, 0x0001_0009]);
        assert_eq!(done.sq_head, 5);
        assert_eq!(done.sq_id, 1);
        assert_eq!(done.cid, 9);
        assert!(done.phase);
        assert!(done.ok());
        // SCT 0, SC 0x80 (LBA out of range), phase 0.
        let failed = Completion::parse([0, 0, 0, (0x80 << 17) | 3]);
        assert!(!failed.phase);
        assert!(!failed.ok());
        assert_eq!(failed.status_parts(), (0, 0x80));
        // SCT 1 (command specific), SC 2.
        let specific = Completion::parse([0, 0, 0, ((1 << 8 | 2) << 17) | (1 << 16)]);
        assert_eq!(specific.status_parts(), (1, 2));
        assert!(specific.phase);
    }
}
