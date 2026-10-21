//! Read-only virtio block devices, polled.
//!
//! Each disk gets one staging buffer of [`STAGING_BYTES`], allocated once from
//! contiguous frames and used through the direct map, so the device writes into
//! it in place (see `hal.rs`). Every read is checked against the disk and the
//! buffer before it reaches the driver, which panics on a length that is not a
//! whole number of sectors. Nothing here writes to a disk. The disks go to
//! `block`, which the model loader and the tests reach them through.

use super::hal::KernelHal;
use crate::{block, log, memory};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use mm::PAGE_SIZE;
use mm::dma::{self, ReadError, SECTOR};
use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::pci::PciTransport;

/// One disk's staging buffer, and so the most one read moves: 512 KiB.
pub const STAGING_BYTES: usize = 512 * 1024;
/// Sectors in one full read.
const MAX_SECTORS: u64 = STAGING_BYTES as u64 / SECTOR;
/// The device ID (serial) is at most this long.
const ID_LEN: usize = 20;

type Device = VirtIOBlk<KernelHal, PciTransport>;

pub struct Disk {
    device: Device,
    /// The device ID QEMU sets with `serial=`.
    serial: String,
    sectors: u64,
    read_only: bool,
    /// [`STAGING_BYTES`] of contiguous frames in the direct map, used by this disk
    /// alone and never freed.
    staging: &'static mut [u8],
}

// SAFETY: a Disk is only used under the block list's mutex, by one thread at a
// time; its registers and staging buffer are not tied to a particular CPU.
unsafe impl Send for Disk {}

impl Disk {
    fn new(transport: PciTransport) -> Result<Disk, &'static str> {
        let mut device = Device::new(transport).map_err(|_| "blk: virtio-blk setup failed")?;
        device.disable_interrupts();
        let mut id = [0u8; ID_LEN];
        let serial = match device.device_id(&mut id) {
            Ok(len) => String::from_utf8_lossy(&id[..len.min(ID_LEN)]).into_owned(),
            Err(_) => String::new(),
        };
        let pages = STAGING_BYTES as u64 / PAGE_SIZE;
        let phys = memory::with(|m| m.allocate_contiguous(pages))
            .ok_or("blk: out of frames for a staging buffer")?;
        // SAFETY: the frames were just allocated for this disk alone, the direct map
        // covers them, and they are never freed, so the slice lives for good.
        let staging = unsafe {
            core::slice::from_raw_parts_mut((memory::hhdm() + phys) as *mut u8, STAGING_BYTES)
        };
        Ok(Disk {
            sectors: device.capacity(),
            read_only: device.readonly(),
            device,
            serial,
            staging,
        })
    }
}

impl block::Device for Disk {
    fn kind(&self) -> &'static str {
        "blk"
    }

    fn serial(&self) -> &str {
        &self.serial
    }

    /// Whether the device refuses writes (QEMU's read-only attach).
    fn read_only(&self) -> bool {
        self.read_only
    }

    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn max_sectors(&self) -> u64 {
        MAX_SECTORS
    }

    /// Reads `sectors` sectors from `sector` into the staging buffer.
    fn read(&mut self, sector: u64, sectors: u64) -> Result<&[u8], &'static str> {
        let bytes = dma::check_read(sector, sectors, self.sectors, STAGING_BYTES as u64).map_err(
            |error| match error {
                ReadError::Empty => "blk: a read of no sectors",
                ReadError::TooLong => "blk: a read longer than the staging buffer",
                ReadError::PastEnd => "blk: a read past the end of the disk",
            },
        )?;
        // Both fit: bytes is at most STAGING_BYTES, and sector is below capacity.
        let bytes = bytes as usize;
        let block = usize::try_from(sector).map_err(|_| "blk: sector number out of range")?;
        self.device
            .read_blocks(block, &mut self.staging[..bytes])
            .map_err(|_| "blk: the device failed a read")?;
        Ok(&self.staging[..bytes])
    }
}

/// Sets up each block device the probe found and hands it to `block`.
pub fn init(transports: Vec<PciTransport>) {
    for transport in transports {
        match Disk::new(transport) {
            Ok(disk) => block::register(Box::new(disk)),
            Err(message) => log!("{message}"),
        }
    }
}
