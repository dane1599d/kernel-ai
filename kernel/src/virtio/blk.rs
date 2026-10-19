//! Read-only virtio block devices, polled.
//!
//! Each disk gets one staging buffer of [`STAGING_BYTES`], allocated once from
//! contiguous frames and used through the direct map, so the device writes into
//! it in place (see `hal.rs`). Every read is checked against the disk and the
//! buffer before it reaches the driver, which panics on a length that is not a
//! whole number of sectors. Nothing here writes to a disk.

use super::hal::KernelHal;
use crate::{log, memory};
use alloc::string::String;
use alloc::vec::Vec;
use mm::PAGE_SIZE;
use mm::dma::{self, ReadError, SECTOR};
use spin::Mutex;
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

// SAFETY: a Disk is only used under the DISKS mutex, by one thread at a time; its
// registers and staging buffer are not tied to a particular CPU.
unsafe impl Send for Disk {}

static DISKS: Mutex<Vec<Disk>> = Mutex::new(Vec::new());

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

    /// Whether the device refuses writes (QEMU's read-only attach).
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Size in 512-byte sectors.
    pub fn sectors(&self) -> u64 {
        self.sectors
    }

    /// Reads `sectors` sectors from `sector` into the staging buffer.
    pub fn read(&mut self, sector: u64, sectors: u64) -> Result<&[u8], &'static str> {
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

    /// Passes the first `len` bytes of the disk to `f(offset, bytes)` in order, one
    /// read at a time. An image that is not a whole number of sectors is padded to
    /// one by QEMU; that padding must read as zero and is not passed on.
    pub fn stream(
        &mut self,
        len: u64,
        mut f: impl FnMut(u64, &[u8]) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        if dma::sectors_for(len) > self.sectors {
            return Err("blk: the image is longer than the disk");
        }
        for chunk in dma::chunks(len, MAX_SECTORS) {
            let data = self.read(chunk.sector, chunk.sectors)?;
            // keep <= sectors * SECTOR, the length of `data`.
            let (image, padding) = data.split_at(chunk.keep as usize);
            if padding.iter().any(|&byte| byte != 0) {
                return Err("blk: the padding after the image is not zero");
            }
            f(chunk.offset, image)?;
        }
        Ok(())
    }
}

/// Sets up each block device the probe found, and logs it.
pub fn init(transports: Vec<PciTransport>) {
    for transport in transports {
        match Disk::new(transport) {
            Ok(disk) => {
                log!(
                    "blk: disk {:?}, {} sectors ({} KiB), {}",
                    disk.serial,
                    disk.sectors,
                    disk.sectors / 2,
                    if disk.read_only {
                        "read-only"
                    } else {
                        "writable (used read-only)"
                    }
                );
                DISKS.lock().push(disk);
            }
            Err(message) => log!("{message}"),
        }
    }
}

/// Runs `f` on the disk whose serial is `serial`, if there is one. The disk list
/// stays locked while `f` runs, polled reads and all; today only the boot thread
/// and the self-tests call this, one at a time. A second, concurrent user would
/// need the lock taken per read instead.
pub fn with_disk<R>(serial: &str, f: impl FnOnce(&mut Disk) -> R) -> Option<R> {
    let mut disks = DISKS.lock();
    disks.iter_mut().find(|disk| disk.serial == serial).map(f)
}
