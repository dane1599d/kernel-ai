//! virtio block devices, polled.
//!
//! Each disk gets one staging buffer of [`STAGING_BYTES`], allocated once from
//! contiguous frames and used through the direct map, so the device moves data
//! in and out of it in place (see `hal.rs`). Every read and write is checked
//! against the disk and the buffer before it reaches the driver, which panics on
//! a length that is not a whole number of sectors. The disks go to `block`.

use super::hal::KernelHal;
use crate::{block, log, memory};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use mm::PAGE_SIZE;
use mm::dma::{self, ReadError, SECTOR};
use virtio_drivers::device::blk::VirtIOBlk;
use virtio_drivers::transport::Transport;
use virtio_drivers::transport::pci::PciTransport;

/// One disk's staging buffer, and so the most one read moves: 512 KiB.
pub const STAGING_BYTES: usize = 512 * 1024;
/// Sectors in one full read.
const MAX_SECTORS: u64 = STAGING_BYTES as u64 / SECTOR;
/// The device ID (serial) is at most this long.
const ID_LEN: usize = 20;
/// VIRTIO_BLK_F_FLUSH (VIRTIO 1.3 §5.2.3). virtio-drivers negotiates it when the
/// device offers it, but keeps the negotiated set private, and its `flush`
/// returns Ok without sending anything when it was not negotiated; so the kernel
/// reads the offer itself, before setting the device up (ADR 0004 §6).
const F_FLUSH: u64 = 1 << 9;

type Device = VirtIOBlk<KernelHal, PciTransport>;

pub struct Disk {
    device: Device,
    /// The device ID QEMU sets with `serial=`.
    serial: String,
    sectors: u64,
    read_only: bool,
    /// Whether the device offered FLUSH, and so negotiated it.
    flush: bool,
    /// [`STAGING_BYTES`] of contiguous frames in the direct map, used by this disk
    /// alone and never freed.
    staging: &'static mut [u8],
}

// SAFETY: a Disk is only used under the block list's mutex, by one thread at a
// time; its registers and staging buffer are not tied to a particular CPU.
unsafe impl Send for Disk {}

impl Disk {
    fn new(mut transport: PciTransport) -> Result<Disk, &'static str> {
        let flush = transport.read_device_features() & F_FLUSH != 0;
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
            flush,
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

    /// Copies `data` into the staging buffer and writes it from `sector`.
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), &'static str> {
        if self.read_only {
            return Err("blk: the disk is read-only");
        }
        if !(data.len() as u64).is_multiple_of(SECTOR) {
            return Err("blk: a write of part of a sector");
        }
        let sectors = data.len() as u64 / SECTOR;
        let bytes = dma::check_read(sector, sectors, self.sectors, STAGING_BYTES as u64).map_err(
            |error| match error {
                ReadError::Empty => "blk: a write of no sectors",
                ReadError::TooLong => "blk: a write longer than the staging buffer",
                ReadError::PastEnd => "blk: a write past the end of the disk",
            },
        )? as usize;
        let block = usize::try_from(sector).map_err(|_| "blk: sector number out of range")?;
        self.staging[..bytes].copy_from_slice(data);
        self.device
            .write_blocks(block, &self.staging[..bytes])
            .map_err(|_| "blk: the device failed a write")
    }

    fn flush(&mut self) -> Result<(), &'static str> {
        if !self.flush {
            return Err("blk: the device offers no FLUSH");
        }
        self.device
            .flush()
            .map_err(|_| "blk: the device failed a flush")
    }

    /// virtio-blk says nothing of its cache unless it offers CONFIG_WCE, so the
    /// cache is taken to be volatile, and a write is atomic only per sector.
    fn info(&self) -> block::Info {
        block::Info {
            volatile_cache: true,
            flush: self.flush,
            atomic_sectors: 1,
        }
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
