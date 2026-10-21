//! Read-only block devices, whatever their driver: virtio-blk (QEMU) and NVMe
//! (QEMU, VirtualBox, real PCs). Each driver registers its disks here at boot, and
//! the model loader and the self-tests find them by serial, or the model disk by
//! what it holds.

use crate::log;
use alloc::boxed::Box;
use alloc::vec::Vec;
use mm::dma;
use spin::Mutex;

/// One disk, read in whole 512-byte sectors into a buffer the driver owns.
pub trait Device: Send {
    /// The driver's name, for log lines and tests.
    fn kind(&self) -> &'static str;
    /// The serial the disk was attached with (`serial=` in QEMU).
    fn serial(&self) -> &str;
    /// Size in 512-byte sectors.
    fn sectors(&self) -> u64;
    /// Whether the device itself refuses writes.
    fn read_only(&self) -> bool;
    /// Sectors one [`Device::read`] moves at most.
    fn max_sectors(&self) -> u64;
    /// Reads `sectors` sectors from `sector`; the bytes stay valid until the next
    /// read.
    fn read(&mut self, sector: u64, sectors: u64) -> Result<&[u8], &'static str>;
}

static DISKS: Mutex<Vec<Box<dyn Device>>> = Mutex::new(Vec::new());

/// Adds a disk a driver set up, and logs it.
pub fn register(disk: Box<dyn Device>) {
    log!(
        "{}: disk {:?}, {} sectors ({} KiB), {}",
        disk.kind(),
        disk.serial(),
        disk.sectors(),
        disk.sectors() / 2,
        if disk.read_only() {
            "read-only"
        } else {
            "writable (used read-only)"
        }
    );
    DISKS.lock().push(disk);
}

/// Runs `f` on the first disk whose serial is `serial`, if there is one. The disk
/// list stays locked while `f` runs, polled reads and all; today only the boot
/// thread and the self-tests call this, one at a time.
pub fn with_disk<R>(serial: &str, f: impl FnOnce(&mut dyn Device) -> R) -> Option<R> {
    let mut disks = DISKS.lock();
    disks
        .iter_mut()
        .find(|disk| disk.serial() == serial)
        .map(|disk| f(disk.as_mut()))
}

/// Runs `f` on the model disk: the one with serial `kai-model`, or else the first
/// whose first sector starts with the GGUF magic. VirtualBox gives NVMe disks a
/// serial of its own, so there the content is what tells the model disk apart.
pub fn with_model_disk<R>(f: impl FnOnce(&mut dyn Device) -> R) -> Option<R> {
    let mut disks = DISKS.lock();
    let serial = kabi::model::SERIAL;
    let index = match disks.iter().position(|disk| disk.serial() == serial) {
        Some(index) => index,
        None => {
            let index = disks.iter_mut().position(|disk| {
                disk.read(0, 1)
                    .is_ok_and(|first| first.starts_with(&kabi::model::MAGIC))
            })?;
            let disk = &disks[index];
            log!(
                "block: no {serial} disk; using {} disk {:?}, which holds a GGUF file",
                disk.kind(),
                disk.serial()
            );
            index
        }
    };
    Some(f(disks[index].as_mut()))
}

/// Passes the first `len` bytes of `disk` to `f(offset, bytes)` in order, one read
/// at a time. An image that is not a whole number of sectors is padded to one by
/// QEMU; that padding must read as zero and is not passed on.
pub fn stream(
    disk: &mut dyn Device,
    len: u64,
    mut f: impl FnMut(u64, &[u8]) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    if dma::sectors_for(len) > disk.sectors() {
        return Err("disk: the image is longer than the disk");
    }
    let max = disk.max_sectors();
    for chunk in dma::chunks(len, max) {
        let data = disk.read(chunk.sector, chunk.sectors)?;
        // keep <= sectors * SECTOR, the length of `data`.
        let (image, padding) = data.split_at(chunk.keep as usize);
        if padding.iter().any(|&byte| byte != 0) {
            return Err("disk: the padding after the image is not zero");
        }
        f(chunk.offset, image)?;
    }
    Ok(())
}
