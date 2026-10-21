//! One NVMe controller: its registers in BAR0, the admin queue pair and one I/O
//! queue pair, all polled. Commands go one at a time, each waited on before the
//! next, which is all a boot-time model copy needs. Every decision about what to
//! write comes from the host-tested `nvme` crate; this file does the volatile
//! accesses.

use super::dma::Page;
use crate::hpet;
use nvme::command::{self as cmd, Command, Completion};
use nvme::identify::{self, IDENTIFY_BYTES};
use nvme::regs::{self, Capabilities};
use nvme::{CompletionQueue, SubmissionQueue};

/// Entries per queue: one page of 64-byte commands.
const ENTRIES: u16 = 64;
/// The one I/O queue pair's ID.
const IO_QID: u16 = 1;
/// How long one command may take.
const COMMAND_TIMEOUT_NS: u64 = 5_000_000_000;

/// A queue pair's memory and bookkeeping.
struct Pair {
    qid: u16,
    sq_mem: Page,
    cq_mem: Page,
    sq: SubmissionQueue,
    cq: CompletionQueue,
}

impl Pair {
    fn new(qid: u16, entries: u16) -> Result<Self, &'static str> {
        Ok(Self {
            qid,
            sq_mem: Page::new()?,
            cq_mem: Page::new()?,
            sq: SubmissionQueue::new(entries),
            cq: CompletionQueue::new(entries),
        })
    }
}

pub struct Controller {
    /// BAR0 in the kernel's device window.
    regs: u64,
    caps: Capabilities,
    admin: Pair,
    io: Option<Pair>,
    next_cid: u16,
    /// The page Identify writes into.
    scratch: Page,
}

impl Controller {
    fn read32(&self, offset: usize) -> u32 {
        // SAFETY: `regs` maps BAR0 uncached for at least caps.bar_bytes(2) bytes, and
        // every offset used is a register or doorbell inside that.
        unsafe { ((self.regs + offset as u64) as *const u32).read_volatile() }
    }

    fn write32(&self, offset: usize, value: u32) {
        // SAFETY: as in read32.
        unsafe { ((self.regs + offset as u64) as *mut u32).write_volatile(value) }
    }

    fn write64(&self, offset: usize, value: u64) {
        self.write32(offset, value as u32);
        self.write32(offset + 4, (value >> 32) as u32);
    }

    /// Resets and enables the controller at BAR0 `regs` (mapped for `mapped`
    /// bytes) with an admin queue pair.
    pub fn start(regs: u64, mapped: usize) -> Result<Self, &'static str> {
        // SAFETY: the caller mapped at least the first page of BAR0 at `regs`.
        let cap = unsafe {
            let low = (regs as *const u32).read_volatile();
            let high = ((regs + 4) as *const u32).read_volatile();
            u64::from(low) | (u64::from(high) << 32)
        };
        let caps = Capabilities::parse(cap);
        if !caps.nvm {
            return Err("nvme: the controller lacks the NVM command set");
        }
        if !caps.takes_4k_pages() {
            return Err("nvme: the controller does not take 4 KiB pages");
        }
        if caps.bar_bytes(IO_QID + 1) > mapped {
            return Err("nvme: the doorbells lie past the mapped registers");
        }
        let entries = ENTRIES.min(caps.max_entries.min(u32::from(u16::MAX)) as u16);
        let controller = Self {
            regs,
            caps,
            admin: Pair::new(0, entries)?,
            io: None,
            next_cid: 0,
            scratch: Page::new()?,
        };
        controller.reset(u32::from(entries))?;
        Ok(controller)
    }

    fn wait_ready(&self, ready: bool) -> Result<(), &'static str> {
        let deadline = hpet::nanos() + self.caps.ready_timeout_ms.max(500) * 1_000_000;
        loop {
            let status = self.read32(regs::CSTS);
            if status & regs::CSTS_FATAL != 0 && ready {
                return Err("nvme: the controller reported a fatal status");
            }
            if (status & regs::CSTS_READY != 0) == ready {
                return Ok(());
            }
            if hpet::nanos() > deadline {
                return Err("nvme: the controller did not change its ready state in time");
            }
            core::hint::spin_loop();
        }
    }

    /// Disables the controller, points it at the admin queues and enables it.
    fn reset(&self, entries: u32) -> Result<(), &'static str> {
        let cc = self.read32(regs::CC);
        if cc & regs::CC_ENABLE != 0 {
            self.write32(regs::CC, cc & !regs::CC_ENABLE);
        }
        self.wait_ready(false)?;
        // The driver polls: no vector may fire.
        self.write32(regs::INTMS, u32::MAX);
        self.write32(regs::AQA, regs::admin_queue_attributes(entries));
        self.write64(regs::ASQ, self.admin.sq_mem.phys);
        self.write64(regs::ACQ, self.admin.cq_mem.phys);
        self.write32(regs::CC, regs::enable_configuration());
        self.wait_ready(true)
    }

    /// The version register, as major, minor and tertiary.
    pub fn version(&self) -> (u16, u8, u8) {
        regs::version(self.read32(regs::VS))
    }

    /// CAP.MPSMIN as a power of two: the unit of MDTS.
    pub fn min_page_shift(&self) -> u32 {
        self.caps.page_min_shift
    }

    fn cid(&mut self) -> u16 {
        self.next_cid = self.next_cid.wrapping_add(1);
        self.next_cid
    }

    /// Submits `command` on the admin queue (`io` false) or the I/O queue, and
    /// waits for its completion.
    fn run(&mut self, io: bool, command: Command) -> Result<Completion, &'static str> {
        let regs = self.regs;
        let caps = self.caps;
        let pair = match io {
            true => self.io.as_mut().ok_or("nvme: no I/O queue")?,
            false => &mut self.admin,
        };
        let (slot, tail) = pair.sq.push().ok_or("nvme: the submission queue is full")?;
        let entry = pair.sq_mem.virt + u64::from(slot) * 64;
        for (i, dword) in command.dwords.iter().enumerate() {
            // SAFETY: slot < entries, so the 64-byte entry lies inside the queue's
            // page, which the driver owns.
            unsafe { ((entry + 4 * i as u64) as *mut u32).write_volatile(*dword) };
        }
        let doorbell = regs + caps.sq_tail_doorbell(pair.qid) as u64;
        // SAFETY: the doorbell lies inside the mapped BAR0 (checked in `start`); the
        // entry's writes are ordered before it (x86 keeps stores in order, and the
        // queue page is ordinary write-back memory the device snoops).
        unsafe { (doorbell as *mut u32).write_volatile(u32::from(tail)) };
        let deadline = hpet::nanos() + COMMAND_TIMEOUT_NS;
        loop {
            let at = pair.cq_mem.virt + u64::from(pair.cq.head()) * 16;
            let mut words = [0u32; 4];
            for (i, word) in words.iter_mut().enumerate() {
                // SAFETY: the head is below the entry count, so the 16-byte entry lies
                // inside the completion queue's page.
                *word = unsafe { ((at + 4 * i as u64) as *const u32).read_volatile() };
            }
            let done = Completion::parse(words);
            if pair.cq.is_new(done.phase) {
                let head = pair.cq.pop();
                pair.sq.advance_head(done.sq_head);
                let doorbell = regs + caps.cq_head_doorbell(pair.qid) as u64;
                // SAFETY: as for the tail doorbell.
                unsafe { (doorbell as *mut u32).write_volatile(u32::from(head)) };
                if done.cid != command.cid() {
                    continue;
                }
                if !done.ok() {
                    let (kind, code) = done.status_parts();
                    crate::log!(
                        "nvme: opcode {:#04x} failed with status type {kind} code {code:#04x}",
                        command.dwords[0] & 0xFF
                    );
                    return Err("nvme: a command failed");
                }
                return Ok(done);
            }
            if hpet::nanos() > deadline {
                return Err("nvme: a command timed out");
            }
            core::hint::spin_loop();
        }
    }

    /// Identify with `cns` for `nsid`; the page stays in `scratch`.
    fn identify(&mut self, cns: u32, nsid: u32) -> Result<&[u8], &'static str> {
        let cid = self.cid();
        let page = self.scratch.phys;
        self.run(false, Command::identify(cid, cns, nsid, page))?;
        Ok(self.scratch.bytes(IDENTIFY_BYTES))
    }

    pub fn identify_controller(&mut self) -> Result<identify::Controller, &'static str> {
        let page = self.identify(cmd::CNS_CONTROLLER, 0)?;
        identify::Controller::parse(page).map_err(|_| "nvme: a bad Identify Controller page")
    }

    /// The active namespace IDs, at most `max`.
    pub fn namespaces(&mut self, max: usize) -> Result<alloc::vec::Vec<u32>, &'static str> {
        let page = self.identify(cmd::CNS_ACTIVE_NAMESPACES, 0)?;
        Ok(identify::active_namespaces(page).take(max).collect())
    }

    pub fn identify_namespace(&mut self, nsid: u32) -> Result<identify::Namespace, &'static str> {
        let page = self.identify(cmd::CNS_NAMESPACE, nsid)?;
        identify::Namespace::parse(page).map_err(|_| "nvme: a bad Identify Namespace page")
    }

    /// Creates the I/O queue pair.
    pub fn create_io_queues(&mut self) -> Result<(), &'static str> {
        let entries = ENTRIES.min(self.caps.max_entries.min(u32::from(u16::MAX)) as u16);
        let pair = Pair::new(IO_QID, entries)?;
        let (cq, sq) = (pair.cq_mem.phys, pair.sq_mem.phys);
        let cid = self.cid();
        self.run(
            false,
            Command::create_io_cq(cid, IO_QID, u32::from(entries), cq),
        )?;
        let cid = self.cid();
        self.run(
            false,
            Command::create_io_sq(cid, IO_QID, u32::from(entries), sq, IO_QID),
        )?;
        self.io = Some(pair);
        Ok(())
    }

    /// Reads `blocks` logical blocks from `lba` of `nsid` with the given PRPs.
    pub fn read(
        &mut self,
        nsid: u32,
        lba: u64,
        blocks: u32,
        prp1: u64,
        prp2: u64,
    ) -> Result<(), &'static str> {
        let cid = self.cid();
        self.run(true, Command::read(cid, nsid, lba, blocks, prp1, prp2))
            .map(|_| ())
    }
}
