//! AMD-Vi bring-up (ADR 0007 section 5), v0.3's first step: each IOMMU the
//! IVRS lists gets a device table of pass-through entries and a command
//! buffer, and one COMPLETION_WAIT proves the IOMMU reads its commands.
//! Nothing is translated yet: every device keeps reaching memory as before,
//! and confining DMA waits on spike S07-2. `iommu=off` skips all of it.
//!
//! The formats are `amdvi`'s; this module only maps the registers, gives the
//! IOMMU its memory and writes the registers in order.

use crate::{hpet, log, memory};
use amdvi::command::{Command, CommandRing};
use amdvi::dte::{DeviceEntry, ENTRY_BYTES, MAX_DEVICES, table_bytes};
use amdvi::regs::{self, control};
use core::sync::atomic::{Ordering, fence};
use x86_64::structures::paging::PageTableFlags;

/// The registers this module uses end below 0x2028; the block is 16 KiB.
const MMIO_BYTES: u64 = 0x4000;
/// Commands in the buffer: the smallest ring, one page.
const COMMANDS: u32 = 256;
/// What the completion wait stores, so a store from anything else is not taken for it.
const STORE: u64 = 0x4b41_495f_5741_4954;
/// How long the completion wait may take.
const WAIT_NS: u64 = 100_000_000;
/// The domain every pass-through entry names.
const PASS_THROUGH_DOMAIN: u16 = 1;

/// Reads the IVRS the platform parse kept and brings up each IOMMU in it.
pub fn init(platform: &acpitables::Platform, cmdline: &str) {
    let Some(ivrs) = &platform.ivrs else {
        return;
    };
    let units = ivrs.iommus();
    let plural = if units.len() == 1 { "" } else { "s" };
    log!(
        "iommu: {} unit{plural} in the IVRS ({} entries not read)",
        units.len(),
        ivrs.dropped
    );
    if cmdline.split_whitespace().any(|word| word == "iommu=off") {
        log!("iommu: off on the command line; DMA is not confined");
        return;
    }
    for unit in units {
        let id = unit.device_id;
        let (bus, device, function) = (id >> 8, id >> 3 & 31, id & 7);
        match bring_up(unit.base) {
            Ok(ns) => log!(
                "iommu: {bus:02x}:{device:02x}.{function} at {:#x}: a completion wait came back in {} us; every device passes through",
                unit.base,
                ns / 1000
            ),
            Err(why) => log!(
                "iommu: {bus:02x}:{device:02x}.{function} at {:#x}: {why}; left off",
                unit.base
            ),
        }
    }
}

/// The IOMMU's registers, mapped uncached.
struct Mmio(u64);

impl Mmio {
    fn read(&self, offset: usize) -> u64 {
        // SAFETY: `offset` is one of `regs`' registers, inside the mapped block.
        unsafe { core::ptr::read_volatile((self.0 + offset as u64) as *const u64) }
    }

    fn write(&self, offset: usize, value: u64) {
        // SAFETY: as in `read`.
        unsafe { core::ptr::write_volatile((self.0 + offset as u64) as *mut u64, value) }
    }
}

/// `pages` zeroed contiguous frames, or `None` when memory is short.
fn zeroed(pages: u64) -> Option<u64> {
    let phys = memory::with(|m| m.allocate_contiguous(pages))?;
    // SAFETY: the frames were just allocated, so nothing else uses them, and
    // the direct map covers every frame the allocator gives.
    unsafe {
        core::ptr::write_bytes(
            (memory::hhdm() + phys) as *mut u8,
            0,
            (pages * 4096) as usize,
        )
    };
    Some(phys)
}

fn release(phys: u64, pages: u64) {
    if let Err(why) = memory::with(|m| m.free_contiguous(phys, pages)) {
        log!("iommu: frames at {phys:#x} not freed: {why}");
    }
}

/// Gives the IOMMU at `base` its device table and command buffer, turns it
/// on, and waits for one COMPLETION_WAIT: how long that took, in
/// nanoseconds. On an error the IOMMU is left off and its memory freed.
fn bring_up(base: u64) -> Result<u64, &'static str> {
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_EXECUTE;
    let mmio = Mmio(memory::map_physical(base, MMIO_BYTES, flags)?);
    if mmio.read(regs::CONTROL) & control::IOMMU_EN != 0 {
        return Err("already on when the kernel started");
    }
    let table = table_bytes((MAX_DEVICES - 1) as u16);
    let table_pages = (table / 4096) as u64;
    let devices = zeroed(table_pages).ok_or("no memory for the device table")?;
    let entry = DeviceEntry::pass_through(PASS_THROUGH_DOMAIN)
        .to_bytes()
        .expect("a pass-through entry encodes");
    let at = (memory::hhdm() + devices) as *mut u8;
    for device in 0..MAX_DEVICES {
        // SAFETY: entry `device` lies inside the table just allocated.
        unsafe {
            core::ptr::copy_nonoverlapping(
                entry.as_ptr(),
                at.add(device * ENTRY_BYTES),
                ENTRY_BYTES,
            )
        };
    }
    let mut ring = CommandRing::new(COMMANDS).expect("256 commands is a ring");
    let (Some(commands), Some(store)) = (zeroed(1), zeroed(1)) else {
        release(devices, table_pages);
        return Err("no memory for the command buffer");
    };
    let free = || {
        release(devices, table_pages);
        release(commands, 1);
        release(store, 1);
    };
    // The IOMMU reads the table and the ring once it is on: written first.
    fence(Ordering::SeqCst);
    let (Some(table_base), Some(ring_base)) = (
        regs::device_table_base(devices, table),
        regs::ring_base(commands, COMMANDS),
    ) else {
        free();
        return Err("its memory does not fit the base registers");
    };
    mmio.write(regs::DEVICE_TABLE_BASE, table_base);
    mmio.write(regs::COMMAND_BUFFER_BASE, ring_base);
    mmio.write(regs::COMMAND_HEAD, 0);
    mmio.write(regs::COMMAND_TAIL, 0);
    let before = mmio.read(regs::CONTROL);
    mmio.write(
        regs::CONTROL,
        before | control::IOMMU_EN | control::CMD_BUF_EN,
    );
    let wait = Command::CompletionWait {
        store: Some((store, STORE)),
        interrupt: false,
        flush: false,
    };
    // SAFETY: the ring's page was allocated above and is the IOMMU's and this
    // function's alone.
    let ring_bytes = unsafe {
        core::slice::from_raw_parts_mut((memory::hhdm() + commands) as *mut u8, ring.bytes())
    };
    let pushed = ring.push(ring_bytes, mmio.read(regs::COMMAND_HEAD), &wait);
    let started = hpet::nanos();
    let stored = (memory::hhdm() + store) as *const u64;
    let came = pushed.is_ok() && {
        fence(Ordering::SeqCst);
        mmio.write(regs::COMMAND_TAIL, ring.tail());
        loop {
            // SAFETY: the store page was allocated above; the IOMMU writes it.
            if unsafe { core::ptr::read_volatile(stored) } == STORE {
                break true;
            }
            if hpet::nanos() - started > WAIT_NS {
                break false;
            }
            core::hint::spin_loop();
        }
    };
    let took = hpet::nanos() - started;
    if !came {
        mmio.write(regs::CONTROL, before);
        free();
        return Err(match pushed {
            Err(_) => "its command buffer took no command",
            Ok(()) => "no completion wait within 100 ms",
        });
    }
    // On for good: the table, the ring and the store page stay the IOMMU's.
    Ok(took)
}
