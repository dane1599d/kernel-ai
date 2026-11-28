//! AMD-Vi bring-up (ADR 0007 section 5), v0.3's first step. By default the
//! kernel only reads the IVRS and says how many IOMMUs it lists, and touches
//! no IOMMU register: until spike S07-2, and a check of `amdvi`'s layouts
//! against the specification, a wrong device table bit with IommuEn on could
//! block a real machine's disk, network or interrupts (the decisions row of
//! 2026-11-28, "amdvi-clean-room").
//!
//! With `iommu=on`, each IOMMU gets a device table of pass-through entries
//! (IV clear, so interrupts pass as sent), a command buffer and an event
//! log, and one COMPLETION_WAIT proves the IOMMU reads its commands; a unit
//! that fails is turned off again. Nothing is translated: every device keeps
//! reaching memory as before. `iommu=off` says so and does nothing.
//!
//! The formats are `amdvi`'s; this module only maps the registers, gives the
//! IOMMU its memory and writes the registers in order. `iommu=check` (the
//! `iommu` boot case) then proves the command and event-log formats against
//! the IOMMU itself ([`check`]); it ends on a deliberately bad command, which
//! real hardware answers by halting the command buffer, so it is for QEMU.

use crate::{hpet, log, memory};
use alloc::vec::Vec;
use amdvi::command::{COMMAND_BYTES, Command, CommandRing, Pages};
use amdvi::dte::{DeviceEntry, ENTRY_BYTES, MAX_DEVICES, table_bytes};
use amdvi::event::{Event, EventLog};
use amdvi::regs::{self, control, status};
use core::sync::atomic::{Ordering, fence};
use x86_64::structures::paging::PageTableFlags;

/// The registers this module uses end below 0x2028; the block is 16 KiB.
const MMIO_BYTES: u64 = 0x4000;
/// Commands in the buffer, and entries in the event log: the smallest
/// rings, one page each.
const COMMANDS: u32 = 256;
const EVENTS: u32 = 256;
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
    let said = |word: &str| cmdline.split_whitespace().any(|w| w == word);
    if said("iommu=off") {
        log!("iommu: off on the command line; DMA is not confined");
        return;
    }
    if !said("iommu=on") {
        log!("iommu: left off until iommu=on (spike S07-2); DMA is not confined");
        return;
    }
    for unit in units {
        let id = unit.device_id;
        let (bus, device, function) = (id >> 8, id >> 3 & 31, id & 7);
        match bring_up(unit.base) {
            Ok((mut on, ns)) => {
                log!(
                    "iommu: {bus:02x}:{device:02x}.{function} at {:#x}: a completion wait came back in {} us; every device passes through",
                    unit.base,
                    ns / 1000
                );
                if said("iommu=check") {
                    check(&mut on, id);
                }
            }
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

/// An IOMMU that is on: its registers and the memory it was given.
struct Unit {
    mmio: Mmio,
    ring: CommandRing,
    commands: u64,
    events: u64,
    store: u64,
}

impl Unit {
    fn ring_bytes(&mut self) -> &mut [u8] {
        // SAFETY: the ring's page is the IOMMU's and this unit's alone.
        unsafe {
            core::slice::from_raw_parts_mut(
                (memory::hhdm() + self.commands) as *mut u8,
                self.ring.bytes(),
            )
        }
    }

    /// Writes `command` at the tail, without moving the tail register.
    fn stage(&mut self, command: &Command) -> Result<(), &'static str> {
        let head = self.mmio.read(regs::COMMAND_HEAD);
        let mut ring = self.ring.clone();
        ring.push(self.ring_bytes(), head, command)
            .map_err(|_| "its command buffer took no command")?;
        self.ring = ring;
        Ok(())
    }

    /// Moves the tail register to the ring's tail: the IOMMU runs what was staged.
    fn run(&self) {
        fence(Ordering::SeqCst);
        self.mmio.write(regs::COMMAND_TAIL, self.ring.tail());
    }

    fn stored(&self, offset: u64) -> u64 {
        // SAFETY: the store page is this unit's; the IOMMU writes its first word.
        unsafe { core::ptr::read_volatile((memory::hhdm() + self.store + offset) as *const u64) }
    }

    /// A COMPLETION_WAIT after what was staged that stores `data`: how long
    /// it took to land, in nanoseconds.
    fn wait(&mut self, data: u64, interrupt: bool) -> Result<u64, &'static str> {
        self.stage(&Command::CompletionWait {
            store: Some((self.store, data)),
            interrupt,
            flush: false,
        })?;
        let started = hpet::nanos();
        self.run();
        while self.stored(0) != data {
            if hpet::nanos() - started > WAIT_NS {
                return Err("no completion wait within 100 ms");
            }
            core::hint::spin_loop();
        }
        Ok(hpet::nanos() - started)
    }

    /// The events logged since the last call, the head moved past them.
    fn events(&self) -> Result<Vec<Event>, &'static str> {
        let log = EventLog::new(EVENTS).expect("256 entries is a log");
        // SAFETY: the log's page is the IOMMU's to write and this unit's to read.
        let bytes = unsafe {
            core::slice::from_raw_parts((memory::hhdm() + self.events) as *const u8, log.bytes())
        };
        let head = self.mmio.read(regs::EVENT_HEAD);
        let tail = self.mmio.read(regs::EVENT_TAIL);
        let (events, head) = log
            .drain(bytes, head, tail)
            .map_err(|_| "its event log pointers are past the log")?;
        self.mmio.write(regs::EVENT_HEAD, head);
        Ok(events)
    }
}

/// Gives the IOMMU at `base` its device table, command buffer and event
/// log, turns it on, and waits for one COMPLETION_WAIT: the unit, and how
/// long that took in nanoseconds. On an error the IOMMU is left off and its
/// memory freed.
fn bring_up(base: u64) -> Result<(Unit, u64), &'static str> {
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
    let (Some(commands), Some(events), Some(store)) = (zeroed(1), zeroed(1), zeroed(1)) else {
        release(devices, table_pages);
        return Err("no memory for the command buffer and event log");
    };
    let free = || {
        release(devices, table_pages);
        release(commands, 1);
        release(events, 1);
        release(store, 1);
    };
    let (Some(table_base), Some(ring_base), Some(log_base)) = (
        regs::device_table_base(devices, table),
        regs::ring_base(commands, COMMANDS),
        regs::ring_base(events, EVENTS),
    ) else {
        free();
        return Err("its memory does not fit the base registers");
    };
    // The IOMMU reads the table and the ring once it is on: written first.
    fence(Ordering::SeqCst);
    mmio.write(regs::DEVICE_TABLE_BASE, table_base);
    mmio.write(regs::COMMAND_BUFFER_BASE, ring_base);
    mmio.write(regs::EVENT_LOG_BASE, log_base);
    for pointer in [
        regs::COMMAND_HEAD,
        regs::COMMAND_TAIL,
        regs::EVENT_HEAD,
        regs::EVENT_TAIL,
    ] {
        mmio.write(pointer, 0);
    }
    let before = mmio.read(regs::CONTROL);
    let on = control::IOMMU_EN | control::CMD_BUF_EN | control::EVENT_LOG_EN;
    mmio.write(regs::CONTROL, before | on);
    let mut unit = Unit {
        mmio,
        ring: CommandRing::new(COMMANDS).expect("256 commands is a ring"),
        commands,
        events,
        store,
    };
    match unit.wait(STORE, false) {
        // On for good: the table, the rings and the store page stay the IOMMU's.
        Ok(ns) => Ok((unit, ns)),
        Err(why) => {
            unit.mmio.write(regs::CONTROL, before);
            free();
            Err(why)
        }
    }
}

/// `iommu=check`: the command and event-log formats proved against the
/// IOMMU, one line each. A check the specification settles fails the boot
/// case when it does not hold; a status bit QEMU may not model is a `note`.
fn check(unit: &mut Unit, own: u16) {
    let mut failed = 0;
    let mut say = |name: &str, result: Result<(), &str>| match result {
        Ok(()) => log!("iommu-check: {name} ok"),
        Err(why) => {
            failed += 1;
            log!("iommu-check: {name} FAILED: {why}");
        }
    };
    let state = unit.mmio.read(regs::STATUS);
    say(
        "status",
        match state {
            s if s & status::CMD_BUF_RUN == 0 => Err("CmdBufRun is clear"),
            s if s & status::EVENT_LOG_RUN == 0 => Err("EventLogRun is clear"),
            s if s & status::EVENT_OVERFLOW != 0 => Err("EventOverflow is set"),
            _ => Ok(()),
        },
    );
    // A store lands as all 64 bits, at its address and nowhere else.
    const SENTINEL: u64 = 0x5a5a_5a5a_5a5a_5a5a;
    const DATA: u64 = 0x1122_3344_5566_7788;
    // SAFETY: the store page is this unit's; the IOMMU writes only its first word.
    unsafe { core::ptr::write_volatile((memory::hhdm() + unit.store + 8) as *mut u64, SENTINEL) };
    say(
        "completion wait store",
        unit.wait(DATA, false)
            .and_then(|_| match (unit.stored(0), unit.stored(8)) {
                (DATA, SENTINEL) => Ok(()),
                (DATA, _) => Err("the word after the store changed"),
                _ => Err("the stored data differs"),
            }),
    );
    // Invalidations of every shape complete, and log nothing.
    let invalidations = [
        Command::InvalidateDevtabEntry { device: own },
        Command::InvalidateIommuPages {
            domain: PASS_THROUGH_DOMAIN,
            pages: Pages::One(0x1000),
            pde: false,
        },
        Command::InvalidateIommuPages {
            domain: PASS_THROUGH_DOMAIN,
            pages: Pages::Range {
                iova: 0x10_0000,
                count: 16,
            },
            pde: true,
        },
        Command::InvalidateIommuPages {
            domain: PASS_THROUGH_DOMAIN,
            pages: Pages::All,
            pde: true,
        },
    ];
    let staged = invalidations.iter().try_for_each(|c| unit.stage(c));
    say(
        "invalidations",
        staged
            .and_then(|()| unit.wait(DATA + 1, false))
            .and_then(|_| unit.events())
            .and_then(|events| match events.is_empty() {
                true => Ok(()),
                false => {
                    log!("iommu-check: events after the invalidations: {events:?}");
                    Err("they logged events")
                }
            }),
    );
    // A wait that asks for its interrupt: ComWaitInt, cleared by writing 1.
    if unit.wait(DATA + 2, true).is_ok() {
        let set = unit.mmio.read(regs::STATUS) & status::COM_WAIT_INT != 0;
        unit.mmio.write(regs::STATUS, status::COM_WAIT_INT);
        let cleared = unit.mmio.read(regs::STATUS) & status::COM_WAIT_INT == 0;
        log!(
            "iommu-check: note: ComWaitInt after a wait with I set: set {set}, cleared by a 1 {cleared}"
        );
    }
    // Last, as real hardware halts its command buffer on it: a command of
    // opcode 0, which is reserved, logged as ILLEGAL_COMMAND_ERROR at the
    // command's own address.
    let at = unit.commands + unit.ring.tail();
    let bad = unit
        .stage(&Command::InvalidateDevtabEntry { device: own })
        .map(|()| {
            let slot = (unit.ring.tail() + unit.ring.bytes() as u64 - COMMAND_BYTES as u64)
                % unit.ring.bytes() as u64;
            unit.ring_bytes()[slot as usize..slot as usize + COMMAND_BYTES].fill(0);
            unit.run();
        });
    let started = hpet::nanos();
    let logged = bad.and_then(|()| {
        loop {
            if unit.mmio.read(regs::EVENT_TAIL) != unit.mmio.read(regs::EVENT_HEAD) {
                break unit.events();
            }
            if hpet::nanos() - started > WAIT_NS {
                break Err("no event within 100 ms");
            }
            core::hint::spin_loop();
        }
    });
    say(
        "illegal command event",
        logged.and_then(|events| match events.as_slice() {
            [Event::IllegalCommand { address }] if *address == at => Ok(()),
            other => {
                log!("iommu-check: the events: {other:?}, the command at {at:#x}");
                Err("not one ILLEGAL_COMMAND_ERROR at the command's address")
            }
        }),
    );
    let state = unit.mmio.read(regs::STATUS);
    log!(
        "iommu-check: note: after the event, EventLogInt {} CmdBufRun {}",
        state & status::EVENT_LOG_INT != 0,
        state & status::CMD_BUF_RUN != 0
    );
    match failed {
        0 => log!("iommu-check: every check passed"),
        n => log!("iommu-check: {n} check(s) FAILED"),
    }
}
