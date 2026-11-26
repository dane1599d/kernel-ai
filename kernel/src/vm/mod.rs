//! Spike S06-3 (ADR 0006): our own throwaway hypervisor. One vCPU on AMD SVM
//! with nested paging boots a stock Debian bzImage and an initramfs, both
//! modules in the boot bundle (`guest/bzImage`, `guest/initrd`, and extra
//! command-line words in `guest/cmdline`), fetched in CI and never committed.
//! Devices: COM1, the PIT, the PIC and CMOS, all port I/O; an MP table; no
//! ACPI, no PCI, no virtio. The guest runs with `nolapic noapic`, so the
//! LAPIC and IOAPIC pages are plain RAM holding the two registers Linux
//! reads while it parses the MP table. Never merged: `spike/our-hypervisor`.
//!
//! The measures, as `s06-3: measure ...` lines (host clock): boot to the
//! kernel's "Run /init" line, to `/init`'s marker, `dav1d`'s decode time,
//! and the mean port-I/O round trip; `s063probe` logs its own median.

mod devices;
mod linux;
mod vcpu;

use crate::{hpet, log, memory};
use alloc::format;
use alloc::vec::Vec;
use svm::npt::{Access, Frames, Region, Table, Tables};

const GUEST_MIB: u64 = 512;
const PAGE: u64 = 4096;

/// What every boot gets; `guest/cmdline` adds to it.
const CMDLINE: &str = "console=ttyS0,115200 earlyprintk=serial,ttyS0,115200 nolapic noapic acpi=off pci=off nosmp tsc=reliable clocksource=tsc i8042.noaux i8042.nopnp 8250.nr_uarts=1 mitigations=off panic=-1 rdinit=/init";

/// Guest RAM and the two APIC pages: one run of host frames.
struct Run {
    base: u64,
    bytes: u64,
}

impl Frames for Run {
    fn run(&self, offset: u64, bytes: u64) -> Option<u64> {
        (offset + bytes <= self.bytes).then_some(self.base + offset)
    }
}

/// The nested table's pages, each a zeroed frame.
struct Pool(Vec<u64>);

impl Tables for Pool {
    fn alloc(&mut self) -> Option<u64> {
        let page = memory::with(|m| m.allocate_zeroed_frame())?;
        self.0.push(page);
        Some(page)
    }

    fn get(&self, phys: u64) -> Option<&Table> {
        // SAFETY: a page `alloc` gave, zeroed then and reachable through the
        // direct map; only this pool hands out references to it.
        self.0.contains(&phys).then(|| unsafe { &*((memory::hhdm() + phys) as *const Table) })
    }

    fn get_mut(&mut self, phys: u64) -> Option<&mut Table> {
        // SAFETY: as `get`, with `&mut self` keeping the reference unique.
        self.0.contains(&phys).then(|| unsafe { &mut *((memory::hhdm() + phys) as *mut Table) })
    }
}

fn frames(count: u64) -> Result<u64, &'static str> {
    let base = memory::with(|m| m.allocate_aligned(count, 1)).ok_or("s06-3: out of frames")?;
    // SAFETY: frames just allocated, reachable through the direct map.
    unsafe { core::ptr::write_bytes((memory::hhdm() + base) as *mut u8, 0, (count * PAGE) as usize) };
    Ok(base)
}

/// The TSC's rate, timed against the host clock over 50 ms.
fn tsc_khz() -> u32 {
    let (t0, c0) = (hpet::nanos(), unsafe { core::arch::x86_64::_rdtsc() });
    while hpet::nanos() - t0 < 50_000_000 {
        core::hint::spin_loop();
    }
    let (t1, c1) = (hpet::nanos(), unsafe { core::arch::x86_64::_rdtsc() });
    ((c1 - c0) as u128 * 1_000_000 / (t1 - t0) as u128) as u32
}

/// The TSC's rate (kHz), set once by `run` before the guest starts.
static TSC_KHZ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Host nanoseconds from the TSC. The run loop reads the clock on every
/// exit, and under KVM the kernel's clock is the HPET, an MMIO exit to QEMU
/// of its own (about 10 µs on the nested runners).
pub fn nanos() -> u64 {
    let khz = TSC_KHZ.load(core::sync::atomic::Ordering::Relaxed).max(1);
    // SAFETY: RDTSC has no preconditions.
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    (tsc as u128 * 1_000_000 / khz as u128) as u64
}

fn put32(ram: &mut [u8], at: u64, v: u32) {
    ram[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn run() -> Result<(), &'static str> {
    if crate::smp::online() != 1 {
        return Err("s06-3: boot with --cpus 1: SVM is turned on for one CPU only");
    }
    let find = |name| crate::bootfiles::find(name);
    let bzimage = find("guest/bzImage").ok_or("s06-3: no guest/bzImage in the bundle")?;
    let initrd = find("guest/initrd").ok_or("s06-3: no guest/initrd in the bundle")?;
    let extra = find("guest/cmdline")
        .and_then(|b| core::str::from_utf8(b).ok())
        .unwrap_or("")
        .trim();
    let khz = tsc_khz();
    TSC_KHZ.store(khz as u64, core::sync::atomic::Ordering::Relaxed);
    let cmdline = format!("{CMDLINE} tsc_early_khz={khz} {extra}");

    let ram_bytes = GUEST_MIB << 20;
    let shm = ram_bytes + 2 * PAGE;
    let base = memory::with(|m| m.allocate_aligned((ram_bytes >> 12) + 512, 512))
        .ok_or("s06-3: no contiguous run for guest RAM")?;
    // SAFETY: the run was just allocated for the guest alone, and the
    // direct map reaches all of it.
    let ram = unsafe { core::slice::from_raw_parts_mut((memory::hhdm() + base) as *mut u8, shm as usize) };
    ram.fill(0);
    // IOAPIC: IOWIN reads its version (0x11, 24 pins) whatever IOREGSEL
    // says. LAPIC: ID 0 and version 0x14 with 6 LVTs.
    put32(ram, ram_bytes + 0x10, 0x0017_0011);
    put32(ram, ram_bytes + PAGE + 0x30, 0x0005_0014);
    let entry = linux::load(&mut ram[..ram_bytes as usize], bzimage, initrd, &cmdline)?;

    let refused = |_| "s06-3: guest RAM's layout was refused";
    let mut layout = svm::Layout::new(1 << 32, shm).map_err(refused)?;
    let mmio = Access {
        write: true,
        execute: false,
    };
    for (gpa, bytes, offset, access) in [
        (0, ram_bytes, 0, Access::RAM),
        (linux::IOAPIC, PAGE, ram_bytes, mmio),
        (linux::LAPIC, PAGE, ram_bytes + PAGE, mmio),
    ] {
        layout
            .add(Region {
                gpa,
                bytes,
                offset,
                access,
            })
            .map_err(refused)?;
    }
    let mut pool = Pool(Vec::new());
    let root = pool.alloc().ok_or("s06-3: out of frames")?;
    layout
        .map(&Run { base, bytes: shm }, &mut pool, root)
        .map_err(refused)?;
    let ncr3 = svm::npt::ncr3(root).ok_or("s06-3: bad nested root")?;

    let pages = vcpu::Pages {
        vmcb: frames(1)?,
        host: frames(1)?,
        hsave: frames(1)?,
        iopm: frames(3)?,
        msrpm: frames(2)?,
    };
    let nrips = vcpu::enable(pages.hsave)?;
    // SAFETY: a zeroed page this vCPU alone uses as its VMCB.
    let vmcb = unsafe { &mut *((memory::hhdm() + pages.vmcb) as *mut svm::Vmcb) };
    log!(
        "s06-3: guest starts: {GUEST_MIB} MiB, {} nested table pages, nrips {nrips}, TSC {khz} kHz; cmdline: {cmdline}",
        pool.0.len()
    );
    // The guest's vector state lives in this thread's registers: saved and
    // restored at every switch, as a ring-3 thread's is.
    crate::fpu::enter_ring3();
    x86_64::instructions::interrupts::enable();
    let mut dev = devices::Devices::new(khz);
    let mut vcpu = vcpu::Vcpu::new(vmcb, pages, &layout, ncr3, nrips, &entry);
    let end = vcpu.run(&mut dev);
    crate::fpu::leave_ring3();
    report(&vcpu, end)
}

fn report(vcpu: &vcpu::Vcpu, end: Result<vcpu::End, &'static str>) -> Result<(), &'static str> {
    let s = &vcpu.stats;
    let t = &vcpu.times;
    let ms = |ns: u64| ns / 1_000_000;
    log!(
        "s06-3: {} exits: {} port I/O, {} host interrupts, {} HLT, {} CPUID, {} MSR, {} other",
        s.exits,
        s.io,
        s.intr,
        s.hlt,
        s.cpuid,
        s.msr,
        s.other
    );
    if let Some(at) = t.run_init_line {
        log!("s06-3: measure boot-to-init {} ms (the kernel's \"Run /init\" line)", ms(at));
    }
    if let Some(at) = t.init {
        log!("s06-3: measure boot-to-init-marker {} ms (/init's first line)", ms(at));
    }
    if let Some((from, to)) = t.decode {
        log!("s06-3: measure dav1d {} ms", ms(to - from));
    }
    if let Some((from, to, n)) = t.bench.filter(|b| b.2 > 0) {
        log!(
            "s06-3: measure pio-roundtrip mean {} ns over {n} (host clock, loop included)",
            (to - from) / n
        );
    }
    match end? {
        vcpu::End::Done if t.init.is_some() => {
            log!("s06-3: done");
            Ok(())
        }
        vcpu::End::Done => Err("s06-3: the guest finished without reaching init"),
        vcpu::End::Halted => Err("s06-3: the guest halted for good before its done marker"),
    }
}
