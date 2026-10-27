//! Block devices, whatever their driver: virtio-blk (QEMU) and NVMe (QEMU,
//! VirtualBox, real PCs). Each driver registers its disks here at boot, and the
//! model loader, the self-tests and (from P2) the file server find them by serial,
//! or the model disk by what it holds.
//!
//! Once every driver has registered its disks, [`scan`] reads each disk's GPT
//! with `diskmap` and finds `kai-data` and `kai-cache` by partition type GUID,
//! never by serial (ADR 0004 §5); [`with_partition`] reaches one.
//!
//! A disk is writable only if a write can be made durable (ADR 0004 §6): one
//! whose cache may hold writes back must also offer a flush, or it is refused
//! read-write. Each disk has its own lock, so one disk's slow I/O does not hold
//! up another's.

use crate::log;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use mm::dma;
use spin::Mutex;

/// What a disk tells about writing to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// A completed write may sit in a volatile cache until a flush.
    pub volatile_cache: bool,
    /// The device takes a flush that makes completed writes durable.
    pub flush: bool,
    /// Sectors a write is guaranteed to land whole or not at all, at least 1.
    pub atomic_sectors: u64,
}

/// One disk, moved in whole 512-byte sectors through a buffer the driver owns.
pub trait Device: Send {
    /// The driver's name, for log lines and tests.
    fn kind(&self) -> &'static str;
    /// The serial the disk was attached with (`serial=` in QEMU).
    fn serial(&self) -> &str;
    /// Size in 512-byte sectors.
    fn sectors(&self) -> u64;
    /// Whether the disk refuses writes: the device's own setting, or the block
    /// layer's when the device cannot make a write durable.
    fn read_only(&self) -> bool;
    /// Sectors one [`Device::read`] or [`Device::write`] moves at most.
    fn max_sectors(&self) -> u64;
    /// Reads `sectors` sectors from `sector`; the bytes stay valid until the next
    /// call on the disk.
    fn read(&mut self, sector: u64, sectors: u64) -> Result<&[u8], &'static str>;
    /// Writes `data`, whole sectors and at most [`Device::max_sectors`] of them,
    /// from `sector`. It may sit in a volatile cache until [`Device::flush`].
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), &'static str>;
    /// Makes every write completed so far durable.
    fn flush(&mut self) -> Result<(), &'static str>;
    fn info(&self) -> Info;
}

/// Whether a disk with `info` may be written: its writes can be made durable.
pub fn durable(info: Info) -> bool {
    info.flush || !info.volatile_cache
}

/// A registered disk: the driver's device, read-only when the block layer says so.
struct Disk {
    device: Box<dyn Device>,
    writable: bool,
}

impl Device for Disk {
    fn kind(&self) -> &'static str {
        self.device.kind()
    }
    fn serial(&self) -> &str {
        self.device.serial()
    }
    fn sectors(&self) -> u64 {
        self.device.sectors()
    }
    fn read_only(&self) -> bool {
        !self.writable
    }
    fn max_sectors(&self) -> u64 {
        self.device.max_sectors()
    }
    fn read(&mut self, sector: u64, sectors: u64) -> Result<&[u8], &'static str> {
        self.device.read(sector, sectors)
    }
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), &'static str> {
        if !self.writable {
            return Err("block: the disk is read-only");
        }
        self.device.write(sector, data)
    }
    fn flush(&mut self) -> Result<(), &'static str> {
        if !self.writable {
            return Err("block: the disk is read-only");
        }
        // A disk with no volatile cache has nothing to flush.
        if self.device.info().flush {
            self.device.flush()
        } else {
            Ok(())
        }
    }
    fn info(&self) -> Info {
        self.device.info()
    }
}

/// A disk in the list: its serial, kept outside its lock so finding a disk never
/// waits on another's I/O.
struct Entry {
    serial: String,
    disk: Arc<Mutex<Disk>>,
}

/// The disks, each behind its own lock; this list's lock is held only to find one.
static DISKS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// Adds a disk a driver set up, and logs it.
pub fn register(device: Box<dyn Device>) {
    let info = device.info();
    let writable = !device.read_only() && durable(info);
    log!(
        "{}: disk {:?}, {} sectors ({} KiB), {}",
        device.kind(),
        device.serial(),
        device.sectors(),
        device.sectors() / 2,
        match (device.read_only(), writable) {
            (true, _) => "read-only",
            (false, true) if info.volatile_cache => "writable, write-back cache with flush",
            (false, true) => "writable, no volatile cache",
            (false, false) => "read-only: a write-back cache with no flush",
        }
    );
    let serial = String::from(device.serial());
    DISKS.lock().push(Entry {
        serial,
        disk: Arc::new(Mutex::new(Disk { device, writable })),
    });
}

fn find(serial: &str) -> Option<Arc<Mutex<Disk>>> {
    DISKS
        .lock()
        .iter()
        .find(|entry| entry.serial == serial)
        .map(|entry| entry.disk.clone())
}

/// Runs `f` on the first disk whose serial is `serial`, if there is one. Only that
/// disk stays locked while `f` runs.
pub fn with_disk<R>(serial: &str, f: impl FnOnce(&mut dyn Device) -> R) -> Option<R> {
    let disk = find(serial)?;
    let mut disk = disk.lock();
    Some(f(&mut *disk))
}

/// Runs `f` on the model disk: the one with serial `kai-model`, or else the first
/// whose first sector starts with the GGUF magic. VirtualBox gives NVMe disks a
/// serial of its own, so there the content is what tells the model disk apart.
pub fn with_model_disk<R>(f: impl FnOnce(&mut dyn Device) -> R) -> Option<R> {
    let serial = kabi::model::SERIAL;
    let disk = match find(serial) {
        Some(disk) => disk,
        None => {
            let disks: Vec<_> = DISKS.lock().iter().map(|e| e.disk.clone()).collect();
            let disk = disks.into_iter().find(|disk| {
                disk.lock()
                    .read(0, 1)
                    .is_ok_and(|first| first.starts_with(&kabi::model::MAGIC))
            })?;
            let locked = disk.lock();
            log!(
                "block: no {serial} disk; using {} disk {:?}, which holds a GGUF file",
                locked.kind(),
                locked.serial()
            );
            drop(locked);
            disk
        }
    };
    let mut disk = disk.lock();
    Some(f(&mut *disk))
}

/// A partition the scan found.
struct Found {
    kind: diskmap::Guid,
    disk: Arc<Mutex<Disk>>,
    first: u64,
    sectors: u64,
}

static PARTITIONS: Mutex<Vec<Found>> = Mutex::new(Vec::new());

/// The partition types the kernel looks for, and their names for log lines.
const KNOWN: [(diskmap::Guid, &str); 2] = [
    (diskmap::KAI_DATA_TYPE, "kai-data"),
    (diskmap::KAI_CACHE_TYPE, "kai-cache"),
];

/// `diskmap` reading through a disk.
struct Reader<'a>(&'a mut dyn Device);

impl diskmap::Disk for Reader<'_> {
    fn sectors(&self) -> u64 {
        self.0.sectors()
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
        let max = self.0.max_sectors() as usize * dma::SECTOR as usize;
        for (index, chunk) in out.chunks_mut(max).enumerate() {
            let at = lba + (index * max / dma::SECTOR as usize) as u64;
            let sectors = (chunk.len() / dma::SECTOR as usize) as u64;
            let data = self.0.read(at, sectors).map_err(|_| diskmap::Error::Read)?;
            chunk.copy_from_slice(&data[..chunk.len()]);
        }
        Ok(())
    }
}

/// Reads every disk's partition table and keeps the `kai-*` partitions, the
/// first of each type. A disk with no GPT, or a bad one, is skipped.
pub fn scan() {
    let disks: Vec<_> = DISKS.lock().iter().map(|e| e.disk.clone()).collect();
    let mut found: Vec<Found> = Vec::new();
    for disk in disks {
        let table = {
            let mut locked = disk.lock();
            let read = diskmap::read(&mut Reader(&mut *locked));
            match read {
                Ok(table) => {
                    if table.from_backup {
                        log!(
                            "block: {} disk {:?}: the primary GPT is bad; read the backup",
                            locked.kind(),
                            locked.serial()
                        );
                    }
                    table
                }
                Err(diskmap::Error::BadPartitions) => {
                    log!(
                        "block: {} disk {:?}: its GPT's partitions are not valid; skipped",
                        locked.kind(),
                        locked.serial()
                    );
                    continue;
                }
                Err(_) => continue,
            }
        };
        for (kind, name) in KNOWN {
            let Some(part) = table.find(kind) else {
                continue;
            };
            if found.iter().any(|f| f.kind == kind) {
                log!("block: a second {name} partition; the first one found is used");
                continue;
            }
            let locked = disk.lock();
            log!(
                "block: {name} is {} disk {:?}, sectors {} to {} ({} MiB)",
                locked.kind(),
                locked.serial(),
                part.first,
                part.last,
                part.sectors() / 2048
            );
            drop(locked);
            found.push(Found {
                kind,
                disk: disk.clone(),
                first: part.first,
                sectors: part.sectors(),
            });
        }
    }
    *PARTITIONS.lock() = found;
}

/// A partition seen as a disk of its own: sector 0 is its first.
struct Part<'a> {
    disk: &'a mut Disk,
    first: u64,
    sectors: u64,
}

impl Part<'_> {
    fn inside(&self, sector: u64, sectors: u64) -> Result<u64, &'static str> {
        match sector.checked_add(sectors) {
            Some(end) if end <= self.sectors => Ok(self.first + sector),
            _ => Err("block: past the end of the partition"),
        }
    }
}

impl Device for Part<'_> {
    fn kind(&self) -> &'static str {
        self.disk.kind()
    }
    fn serial(&self) -> &str {
        self.disk.serial()
    }
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read_only(&self) -> bool {
        self.disk.read_only()
    }
    fn max_sectors(&self) -> u64 {
        self.disk.max_sectors()
    }
    fn read(&mut self, sector: u64, sectors: u64) -> Result<&[u8], &'static str> {
        let at = self.inside(sector, sectors)?;
        self.disk.read(at, sectors)
    }
    fn write(&mut self, sector: u64, data: &[u8]) -> Result<(), &'static str> {
        let at = self.inside(sector, data.len() as u64 / dma::SECTOR)?;
        self.disk.write(at, data)
    }
    fn flush(&mut self) -> Result<(), &'static str> {
        self.disk.flush()
    }
    fn info(&self) -> Info {
        self.disk.info()
    }
}

/// Whether the scan found a partition of type `kind`.
pub fn has_partition(kind: diskmap::Guid) -> bool {
    PARTITIONS.lock().iter().any(|p| p.kind == kind)
}

/// Runs `f` on the partition of type `kind` the scan found, as a disk whose
/// sector 0 is the partition's first. Only its disk stays locked while `f` runs.
pub fn with_partition<R>(kind: diskmap::Guid, f: impl FnOnce(&mut dyn Device) -> R) -> Option<R> {
    let (disk, first, sectors) = PARTITIONS
        .lock()
        .iter()
        .find(|p| p.kind == kind)
        .map(|p| (p.disk.clone(), p.first, p.sectors))?;
    let mut disk = disk.lock();
    let mut part = Part {
        disk: &mut disk,
        first,
        sectors,
    };
    Some(f(&mut part))
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
