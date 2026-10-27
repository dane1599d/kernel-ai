//! NVMe disks, polled: QEMU's `-device nvme`, VirtualBox's NVMe controller, and
//! the SSDs of real PCs (P7). Each active namespace with 512-byte blocks becomes
//! one `block` disk, named by its controller's serial number. Writes go with
//! WRITE, and FLUSH empties the controller's volatile write cache if it has one
//! (VWC).

mod controller;
mod dma;

use crate::{block, log, memory};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use controller::Controller;
use dma::Page;
use mm::PAGE_SIZE;
use mm::dma::{ReadError, SECTOR, check_read};
use nvme::prp;
use spin::Mutex;

/// One disk's staging buffer, and so the most one read moves: 512 KiB.
const STAGING_BYTES: u64 = 512 * 1024;
/// BAR0 bytes mapped: the registers and the first doorbells.
const BAR_MAP_BYTES: u64 = 16 * 1024;
/// Namespaces taken from one controller at most.
const MAX_NAMESPACES: usize = 8;

/// An NVMe controller the PCI probe found, with its BAR0 assigned by firmware.
pub struct Function {
    /// Where it sits on the bus, for log lines.
    pub name: String,
    pub bar: u64,
    pub bar_len: u64,
}

struct Disk {
    controller: Arc<Mutex<Controller>>,
    nsid: u32,
    serial: String,
    sectors: u64,
    write_protected: bool,
    /// Bytes one READ or WRITE command may move: the controller's MDTS, capped by
    /// the staging buffer.
    max_transfer: u64,
    /// The controller has a volatile write cache.
    volatile_cache: bool,
    /// Sectors a write lands whole across a power failure.
    atomic_sectors: u64,
    staging: Page,
    prp_list: Page,
}

impl block::Device for Disk {
    fn kind(&self) -> &'static str {
        "nvme"
    }

    fn serial(&self) -> &str {
        &self.serial
    }

    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read_only(&self) -> bool {
        self.write_protected
    }

    fn max_sectors(&self) -> u64 {
        STAGING_BYTES / SECTOR
    }

    fn read(&mut self, sector: u64, sectors: u64) -> Result<&[u8], &'static str> {
        let bytes =
            check_read(sector, sectors, self.sectors, STAGING_BYTES).map_err(
                |error| match error {
                    ReadError::Empty => "nvme: a read of no sectors",
                    ReadError::TooLong => "nvme: a read longer than the staging buffer",
                    ReadError::PastEnd => "nvme: a read past the end of the disk",
                },
            )?;
        self.transfer(sector, bytes, false)?;
        Ok(self.staging.bytes(bytes as usize))
    }

    /// Copies `data` into the staging buffer and writes it from `sector`.
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), &'static str> {
        if self.write_protected {
            return Err("nvme: the namespace is write-protected");
        }
        if !(data.len() as u64).is_multiple_of(SECTOR) {
            return Err("nvme: a write of part of a sector");
        }
        let bytes = check_read(
            sector,
            data.len() as u64 / SECTOR,
            self.sectors,
            STAGING_BYTES,
        )
        .map_err(|error| match error {
            ReadError::Empty => "nvme: a write of no sectors",
            ReadError::TooLong => "nvme: a write longer than the staging buffer",
            ReadError::PastEnd => "nvme: a write past the end of the disk",
        })?;
        self.staging.bytes_mut(bytes as usize).copy_from_slice(data);
        self.transfer(sector, bytes, true)
    }

    fn flush(&mut self) -> Result<(), &'static str> {
        self.controller.lock().flush(self.nsid)
    }

    /// FLUSH is a mandatory NVM command; without a volatile cache it has nothing
    /// to do.
    fn info(&self) -> block::Info {
        block::Info {
            volatile_cache: self.volatile_cache,
            flush: true,
            atomic_sectors: self.atomic_sectors,
        }
    }
}

impl Disk {
    /// Moves `bytes` between the staging buffer and the disk from `sector`, in
    /// commands of at most `max_transfer`.
    fn transfer(&mut self, sector: u64, bytes: u64, write: bool) -> Result<(), &'static str> {
        let mut done = 0;
        while done < bytes {
            let piece = (bytes - done).min(self.max_transfer);
            let phys = self.staging.phys + done;
            let prps = prp::describe(phys, piece, self.prp_list.phys)
                .map_err(|_| "nvme: a transfer the PRPs cannot describe")?;
            for index in 0..prps.list_len {
                self.prp_list.set_u64(index, prp::list_entry(phys, index));
            }
            let lba = sector + done / SECTOR;
            let blocks = (piece / SECTOR) as u32;
            let mut controller = self.controller.lock();
            if write {
                controller.write(self.nsid, lba, blocks, prps.prp1, prps.prp2)?;
            } else {
                controller.read(self.nsid, lba, blocks, prps.prp1, prps.prp2)?;
            }
            done += piece;
        }
        Ok(())
    }
}

/// Brings up each controller and hands its namespaces to `block`. A controller
/// that fails is logged, and the kernel runs on without it.
pub fn init(functions: Vec<Function>) {
    for function in functions {
        if let Err(message) = start(&function) {
            log!("{message} ({})", function.name);
        }
    }
}

fn start(function: &Function) -> Result<(), &'static str> {
    let mapped = function.bar_len.min(BAR_MAP_BYTES);
    if mapped < PAGE_SIZE {
        return Err("nvme: BAR0 is smaller than a page");
    }
    let regs = memory::map_physical(function.bar, mapped, crate::virtio::hal::DEVICE)?;
    let mut controller = Controller::start(regs, mapped as usize)?;
    let (major, minor, _) = controller.version();
    let id = controller.identify_controller()?;
    let serial = String::from_utf8_lossy(id.serial()).into_owned();
    log!(
        "nvme: {} is {:?}, serial {:?}, NVMe {major}.{minor}, MDTS {}, {}, AWUPF {} blocks",
        function.name,
        String::from_utf8_lossy(id.model()),
        serial,
        id.mdts,
        if id.volatile_cache {
            "volatile write cache"
        } else {
            "no volatile write cache"
        },
        id.atomic_blocks
    );
    controller.create_io_queues()?;
    let namespaces = controller.namespaces(MAX_NAMESPACES)?;
    let mut found = Vec::new();
    for nsid in namespaces {
        let ns = controller.identify_namespace(nsid)?;
        if ns.block_bytes as u64 != SECTOR {
            log!(
                "nvme: namespace {nsid} has {}-byte blocks; only 512 are read",
                ns.block_bytes
            );
            continue;
        }
        found.push((nsid, ns));
    }
    let max_transfer = id.max_transfer(controller.min_page_shift(), STAGING_BYTES);
    let controller = Arc::new(Mutex::new(controller));
    for (nsid, ns) in found {
        log!("nvme: namespace {nsid}, {} blocks of 512 bytes", ns.blocks);
        block::register(Box::new(Disk {
            controller: controller.clone(),
            nsid,
            serial: serial.clone(),
            sectors: ns.blocks,
            write_protected: ns.write_protected,
            max_transfer,
            volatile_cache: id.volatile_cache,
            atomic_sectors: u64::from(id.atomic_blocks),
            staging: Page::contiguous(STAGING_BYTES / PAGE_SIZE)?,
            prp_list: Page::new()?,
        }));
    }
    Ok(())
}
