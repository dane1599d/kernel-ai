//! S07-2 (ADR 0007), throwaway, on `spike/amdvi-qemu` only: a minimal AMD-Vi
//! setup on QEMU's `amd-iommu`. With `amdvi=on`, before any driver binds:
//!
//! - the IOMMU comes from the IVRS table, and its registers are mapped;
//! - one domain gets a 4-level I/O page table of ours ([`spike_s07_2::identity`]):
//!   memory mapped 1:1 in 2 MiB pages, except a 2 MiB canary that nothing maps;
//! - virtio-net, virtio-rng and the xHCI controller get device table entries in
//!   that domain, and every other device stays untranslated (V clear);
//! - the command buffer and the event log are set up, the IOMMU turned on, and a
//!   COMPLETION_WAIT after the invalidations shows the commands ran.
//!
//! `amdvi_tests` then drives DMA through it. Interrupts are not remapped.

use crate::virtio::hal::DEVICE;
use crate::virtio::pci::EcamAccess;
use crate::{hpet, log, memory};
use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};
use spike_s07_2 as fmt;
use spin::Mutex;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::{Command, DeviceFunction, PciRoot};

/// The one domain the spike's devices share.
pub const DOMAIN: u16 = 1;
/// What the canary page holds; a DMA that reached it would change it.
pub const CANARY_BYTE: u8 = 0xC5;
const TWO_MIB_PAGES: u64 = 512;
const WAIT_DATA: u64 = 0x5A17_0C07_0002_0001;
const WAIT_TIMEOUT_NS: u64 = 1_000_000_000;

/// A device placed in the domain.
#[derive(Clone, Copy, Debug)]
pub struct Confined {
    pub driver: &'static str,
    pub id: u16,
    pub at: DeviceFunction,
}

struct Iommu {
    /// The register window, mapped uncached.
    regs: u64,
    cmd: u64,
    cmd_tail: u64,
    evt: u64,
    wait: u64,
    canary: u64,
    confined: Vec<Confined>,
    access: EcamAccess,
}

static IOMMU: Mutex<Option<Iommu>> = Mutex::new(None);
static RNG: Mutex<Option<PciTransport>> = Mutex::new(None);

/// Whether the command line asks for the spike.
pub fn wanted(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|word| word == "amdvi=on")
}

/// Whether the IOMMU was set up.
pub fn active() -> bool {
    IOMMU.lock().is_some()
}

fn virt(phys: u64) -> u64 {
    memory::hhdm() + phys
}

fn zeroed(pages: u64, align: u64) -> Option<u64> {
    let phys = memory::with(|m| m.allocate_aligned(pages, align))?;
    // SAFETY: the frames were just allocated and the direct map covers them.
    unsafe { core::ptr::write_bytes(virt(phys) as *mut u8, 0, (pages * 4096) as usize) };
    Some(phys)
}

impl Iommu {
    fn read(&self, offset: u64) -> u64 {
        // SAFETY: `regs` maps the IOMMU's register window, uncached, and every
        // offset used is inside it and 8-aligned.
        unsafe { ((self.regs + offset) as *const u64).read_volatile() }
    }

    fn write(&self, offset: u64, value: u64) {
        // SAFETY: as in read.
        unsafe { ((self.regs + offset) as *mut u64).write_volatile(value) }
    }

    fn submit(&mut self, command: [u64; 2]) {
        let at = virt(self.cmd + self.cmd_tail) as *mut u64;
        // SAFETY: the command buffer is ours, 4 KiB, and `cmd_tail` is a
        // 16-byte slot in it.
        unsafe {
            at.write_volatile(command[0]);
            at.add(1).write_volatile(command[1]);
        }
        fence(Ordering::SeqCst);
        self.cmd_tail = (self.cmd_tail + fmt::ENTRY_BYTES) % fmt::RING_BYTES;
        self.write(fmt::reg::CMDBUF_TAIL, self.cmd_tail);
    }

    /// Submits a COMPLETION_WAIT and waits for its store: how long it took,
    /// or None if it never came.
    fn complete(&mut self) -> Option<u64> {
        let word = virt(self.wait) as *mut u64;
        // SAFETY: the wait word is ours, in the direct map.
        unsafe { word.write_volatile(0) };
        let start = hpet::nanos();
        self.submit(fmt::completion_wait(self.wait, WAIT_DATA));
        loop {
            // SAFETY: as above; the IOMMU writes it.
            if unsafe { word.read_volatile() } == WAIT_DATA {
                return Some(hpet::nanos() - start);
            }
            if hpet::nanos() - start > WAIT_TIMEOUT_NS {
                return None;
            }
            core::hint::spin_loop();
        }
    }
}

/// Sets the IOMMU up for the devices among `functions` (segment 0's), before
/// any of them is bound. A failure leaves the IOMMU off and every device
/// untranslated.
pub fn start(
    platform: &acpitables::Platform,
    access: EcamAccess,
    functions: &[pci::Function],
) -> Result<(), &'static str> {
    let ivrs = platform
        .ivrs
        .ok_or("amdvi: no IVRS table, so no AMD IOMMU")?;
    let regs = memory::map_physical(ivrs.base, fmt::reg::WINDOW, DEVICE)
        .map_err(|_| "amdvi: the IOMMU's registers cannot be mapped")?;
    let devtab =
        zeroed(fmt::DEVTAB_BYTES / 4096, 1).ok_or("amdvi: no memory for the device table")?;
    let cmd = zeroed(1, 1).ok_or("amdvi: no memory for the command buffer")?;
    let evt = zeroed(1, 1).ok_or("amdvi: no memory for the event log")?;
    let wait = zeroed(1, 1).ok_or("amdvi: no memory for the wait word")?;
    let canary = zeroed(TWO_MIB_PAGES, TWO_MIB_PAGES).ok_or("amdvi: no 2 MiB for the canary")?;
    // SAFETY: the canary is ours and in the direct map.
    unsafe { core::ptr::write_bytes(virt(canary) as *mut u8, CANARY_BYTE, 4096) };
    let mut tables = 0;
    let mut alloc = || {
        tables += 1;
        zeroed(1, 1)
    };
    let mut store = |table: u64, index: usize, entry: u64| {
        // SAFETY: `table` came from `alloc` above, a zeroed page of ours, and
        // `index` is below 512.
        unsafe { (virt(table) as *mut u64).add(index).write_volatile(entry) }
    };
    let top = memory::phys_end();
    let root = fmt::identity(top, canary, &mut alloc, &mut store)
        .ok_or("amdvi: no memory for the I/O page table")?;
    let dte = fmt::dte(root, 4, DOMAIN);
    // The table reads back as the IOMMU will walk it.
    let load = |table: u64, index: usize| {
        // SAFETY: as in store; the walk reads only tables `identity` linked.
        unsafe { (virt(table) as *const u64).add(index).read_volatile() }
    };
    if fmt::translate(dte[0], canary, &load).is_some()
        || fmt::translate(dte[0], 0x10_0000, &load) != Some((0x10_0000, true))
    {
        return Err("amdvi: the I/O page table does not walk as built");
    }
    let mut confined = Vec::new();
    for f in functions {
        let ids = devmatch::Ids {
            vendor: f.vendor,
            device: f.device,
            class: f.class,
        };
        let Some(row) = devmatch::lookup(devmatch::TABLE, ids) else {
            continue;
        };
        if !matches!(row.driver, "virtio-net" | "virtio-rng" | "xhci") {
            continue;
        }
        let (bus, device, function) = (f.address.bus(), f.address.device(), f.address.function());
        let id = fmt::device_id(bus, device, function);
        let entry = virt(devtab + u64::from(id) * fmt::DTE_BYTES) as *mut u64;
        for (i, quad) in dte.iter().enumerate() {
            // SAFETY: the device table is ours, 2 MiB, one 32-byte entry per ID.
            unsafe { entry.add(i).write_volatile(*quad) };
        }
        confined.push(Confined {
            driver: row.driver,
            id,
            at: DeviceFunction {
                bus,
                device,
                function,
            },
        });
    }
    fence(Ordering::SeqCst);
    let mut iommu = Iommu {
        regs,
        cmd,
        cmd_tail: 0,
        evt,
        wait,
        canary,
        confined,
        access,
    };
    iommu.write(fmt::reg::CONTROL, 0);
    iommu.write(fmt::reg::DEVTAB_BASE, fmt::devtab_base(devtab));
    iommu.write(fmt::reg::CMDBUF_BASE, fmt::ring_base(cmd));
    iommu.write(fmt::reg::EVTLOG_BASE, fmt::ring_base(evt));
    for reg in [
        fmt::reg::CMDBUF_HEAD,
        fmt::reg::CMDBUF_TAIL,
        fmt::reg::EVTLOG_HEAD,
        fmt::reg::EVTLOG_TAIL,
    ] {
        iommu.write(reg, 0);
    }
    iommu.write(
        fmt::reg::CONTROL,
        fmt::control::IOMMU_EN | fmt::control::EVENT_LOG_EN | fmt::control::CMD_BUF_EN,
    );
    let ids: Vec<u16> = iommu.confined.iter().map(|c| c.id).collect();
    for id in ids {
        iommu.submit(fmt::invalidate_dte(id));
    }
    iommu.submit(fmt::invalidate_domain(DOMAIN));
    let waited = iommu.complete();
    let status = iommu.read(fmt::reg::STATUS);
    log!(
        "amdvi: IOMMU {:02x}:{:02x}.{} at {:#x} from IVRS, status {status:#x}",
        ivrs.device_id >> 8,
        (ivrs.device_id >> 3) & 0x1F,
        ivrs.device_id & 7,
        ivrs.base
    );
    log!(
        "amdvi: domain {DOMAIN}: {} MiB mapped 1:1 in 2 MiB pages with {tables} tables, \
         all but the canary at {canary:#x}",
        top.div_ceil(1 << 20)
    );
    for c in &iommu.confined {
        log!(
            "amdvi: {} at {} (device {:#06x}) is in domain {DOMAIN}",
            c.driver,
            c.at,
            c.id
        );
    }
    let Some(waited) = waited else {
        iommu.write(fmt::reg::CONTROL, 0);
        return Err("amdvi: COMPLETION_WAIT never stored, so the IOMMU is off");
    };
    log!("amdvi: COMPLETION_WAIT stored after {} us", waited / 1000);
    *IOMMU.lock() = Some(iommu);
    Ok(())
}

/// The devices in the domain.
pub fn confined() -> Vec<Confined> {
    IOMMU
        .lock()
        .as_ref()
        .map_or_else(Vec::new, |iommu| iommu.confined.clone())
}

/// The canary's physical address: 2 MiB that no I/O page table entry maps.
pub fn canary() -> Option<u64> {
    IOMMU.lock().as_ref().map(|iommu| iommu.canary)
}

/// Whether the canary page still holds only [`CANARY_BYTE`].
pub fn canary_intact() -> bool {
    let Some(canary) = canary() else {
        return false;
    };
    // SAFETY: the canary page is ours, in the direct map, 4 KiB.
    let page = unsafe { core::slice::from_raw_parts(virt(canary) as *const u8, 4096) };
    page.iter().all(|&b| b == CANARY_BYTE)
}

/// The events logged since the last call, and the status register.
pub fn events() -> (Vec<fmt::Event>, u64) {
    let guard = IOMMU.lock();
    let Some(iommu) = guard.as_ref() else {
        return (Vec::new(), 0);
    };
    let mut head = iommu.read(fmt::reg::EVTLOG_HEAD) % fmt::RING_BYTES;
    let tail = iommu.read(fmt::reg::EVTLOG_TAIL) % fmt::RING_BYTES;
    let mut events = Vec::new();
    while head != tail {
        let at = virt(iommu.evt + head) as *const u64;
        // SAFETY: the event log is ours, 4 KiB, and `head` a 16-byte slot in it.
        let entry = unsafe { [at.read_volatile(), at.add(1).read_volatile()] };
        events.push(fmt::Event::decode(entry));
        head = (head + fmt::ENTRY_BYTES) % fmt::RING_BYTES;
    }
    iommu.write(fmt::reg::EVTLOG_HEAD, head);
    (events, iommu.read(fmt::reg::STATUS))
}

/// Keeps the virtio-rng the probe found, for the test, instead of ENTROPY.
pub fn hold_rng(transport: PciTransport) {
    *RNG.lock() = Some(transport);
}

pub fn take_rng() -> Option<PciTransport> {
    RNG.lock().take()
}

/// Clears Bus Master on a confined device and reads the command back: the
/// "kill" of its driver.
pub fn bus_master_off(at: DeviceFunction) -> bool {
    let Some(access) = IOMMU.lock().as_ref().map(|iommu| iommu.access) else {
        return false;
    };
    let mut root = PciRoot::new(access);
    let (_, command) = root.get_status_command(at);
    root.set_command(at, command - Command::BUS_MASTER);
    let (_, after) = root.get_status_command(at);
    !after.contains(Command::BUS_MASTER)
}
