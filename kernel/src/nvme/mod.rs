//! Read-only NVMe disks, polled: QEMU's `-device nvme`, VirtualBox's NVMe
//! controller, and the SSDs of real PCs (P7). Each active namespace with 512-byte
//! blocks becomes one `block` disk, named by its controller's serial number.
//! Nothing here writes to a disk: the driver has no write command.

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
    /// Bytes one READ command may move: the controller's MDTS, capped by the
    /// staging buffer.
    max_transfer: u64,
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
            self.controller
                .lock()
                .read(self.nsid, lba, blocks, prps.prp1, prps.prp2)?;
            done += piece;
        }
        Ok(self.staging.bytes(bytes as usize))
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
        "nvme: {} is {:?}, serial {:?}, NVMe {major}.{minor}, MDTS {}",
        function.name,
        String::from_utf8_lossy(id.model()),
        serial,
        id.mdts
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
            staging: Page::contiguous(STAGING_BYTES / PAGE_SIZE)?,
            prp_list: Page::new()?,
        }));
    }
    Ok(())
}
