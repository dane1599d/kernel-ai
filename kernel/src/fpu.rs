//! Vector state for ring 3. Every CPU runs with x87, SSE and AVX on, and a thread
//! that has entered ring 3 has its x87, SSE and AVX registers saved and restored
//! eagerly at every thread switch, with XSAVE64 and XRSTOR64 in the standard format.
//!
//! - The kernel itself is soft-float, so ring 3's vector registers pass untouched
//!   through every interrupt and system call: only a thread switch needs to save
//!   them. Nothing traps a stray vector instruction in ring 0 any more, so every
//!   xtask build disassembles the kernel ELF and fails if it finds one (only this
//!   file's XSAVE and XRSTOR pass).
//! - [`switch`] runs before the stack switch, so a thread's state is in memory before
//!   `finish_switch` clears its ON_CPU flag. A thread migrated to another CPU then
//!   restores exactly what it saved (the threads.rs rule).
//! - A thread's first entry to ring 3 loads the init image instead: every register
//!   zero, the x87 control word 0x037F and MXCSR 0x1F80. No process ever sees the
//!   registers another process, or the firmware, left on the CPU.
//! - Every restore first resets the x87 component. AMD CPUs without XSaveErPtr load
//!   FIP, FDP and FOP only while an x87 exception is pending, and would otherwise keep
//!   the previous thread's.
//! - Each thread slot has a 4 KiB area reserved statically. The size the CPU needs for
//!   the enabled components comes from CPUID.(0xD,0).EBX after XSETBV, and boot fails
//!   if it does not fit.

use crate::{percpu, threads};
use core::arch::x86_64::__cpuid_count;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use sched::{MAX_THREADS, Tid};
use x86_64::instructions::interrupts;

/// Bytes reserved per thread slot. AVX-512's standard format (L6) needs 2,432 on the
/// 7700X; AMX, which needs 8 KiB more, is never turned on.
const AREA_BYTES: usize = 4096;
/// Where MXCSR sits in the legacy region, and its value at reset: every exception masked.
const MXCSR_OFFSET: usize = 24;
const MXCSR_INIT: u32 = 0x1F80;
/// XRSTOR's requested-feature masks: the x87 component alone, and everything XCR0 enables.
const X87_ONLY: u64 = cpufeat::XCR0_X87;
const ALL: u64 = u64::MAX;

#[repr(C, align(64))]
struct Area(UnsafeCell<[u8; AREA_BYTES]>);

// SAFETY: a thread's area is written by XSAVE only on the CPU switching that thread
// out, or by the thread itself before its first ring-3 entry, and read by XRSTOR only
// on a CPU switching it in, after ON_CPU (release/acquire) shows the save finished. So
// no two CPUs ever touch one area at the same time.
unsafe impl Sync for Area {}

static AREAS: [Area; MAX_THREADS] = [const { Area(UnsafeCell::new([0; AREA_BYTES])) }; MAX_THREADS];

/// The init image, never written: XSTATE_BV 0, so XRSTOR puts every component it
/// restores in its initial configuration, and MXCSR 0x1F80, which XRSTOR loads from
/// the legacy region whatever XSTATE_BV says.
#[repr(C, align(64))]
struct Image([u8; AREA_BYTES]);

static INIT: Image = Image(init_image());

const fn init_image() -> [u8; AREA_BYTES] {
    let mut image = [0; AREA_BYTES];
    let mxcsr = MXCSR_INIT.to_le_bytes();
    let mut i = 0;
    while i < mxcsr.len() {
        image[MXCSR_OFFSET + i] = mxcsr[i];
        i += 1;
    }
    image
}

/// Set from a thread's first ring-3 entry until it exits: its vector state is saved.
static IN_RING3: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
/// What the boot CPU chose, stored before any other CPU starts; the others must match.
static XCR0: AtomicU64 = AtomicU64::new(0);
static AREA_SIZE: AtomicUsize = AtomicUsize::new(0);

/// Turns on x87, SSE and AVX state on this CPU, whose CR4.OSXSAVE is already set:
/// XSETBV with the XCR0 `cpufeat` chooses from `supported` (CPUID.(0xD,0) EDX:EAX),
/// then checks the XSAVE area the CPU needs for it. Returns XCR0.
pub fn init_cpu(supported: u64) -> Result<u64, &'static str> {
    let xcr0 = cpufeat::ring3_xcr0(supported).ok_or("cpu: XSAVE cannot hold x87 and SSE state")?;
    // SAFETY: CR4.OSXSAVE is set, and XCR0 names only components the CPU supports,
    // x87 and SSE among them, so XSETBV cannot fault. The kernel is soft-float, so
    // which vector components exist changes nothing it relies on.
    unsafe { xsetbv(xcr0) };
    let reported = __cpuid_count(0xd, 0).ebx;
    let avx = __cpuid_count(0xd, 2);
    let avx_end = avx.ebx.saturating_add(avx.eax);
    let size = cpufeat::xsave_area_bytes(xcr0, reported, avx_end, AREA_BYTES)?;
    if percpu::index() == 0 {
        XCR0.store(xcr0, Ordering::Relaxed);
        AREA_SIZE.store(size, Ordering::Relaxed);
    } else if XCR0.load(Ordering::Relaxed) != xcr0 || AREA_SIZE.load(Ordering::Relaxed) != size {
        return Err("cpu: XCR0 or the XSAVE area size differs from the boot CPU's");
    }
    Ok(xcr0)
}

/// XCR0 on every CPU.
pub fn xcr0() -> u64 {
    XCR0.load(Ordering::Relaxed)
}

/// Bytes of XSAVE area each thread's state takes.
pub fn area_size() -> usize {
    AREA_SIZE.load(Ordering::Relaxed)
}

/// Saves `from`'s ring-3 vector state and loads `to`'s, for each of them that has
/// entered ring 3. Called by `threads::switch` before the stack switch, with
/// interrupts off.
pub fn switch(from: Tid, to: Tid) {
    if IN_RING3[from].load(Ordering::Acquire) {
        // SAFETY: `from` is running on this CPU, so no other CPU touches its area,
        // which is 64-byte aligned and at least the CPU's XSAVE size; XSAVE is on.
        unsafe { xsave(area(from)) };
    }
    if IN_RING3[to].load(Ordering::Acquire) {
        // SAFETY: `to` is not running anywhere (ON_CPU was false when this CPU took
        // it), so its area holds what XSAVE last wrote for it, and nothing writes it
        // now.
        unsafe { restore(area(to)) };
    }
}

/// Gives the calling thread clean vector registers for its first entry to ring 3,
/// and from then on saves and restores them at every switch. Returns with interrupts
/// off, so the init state is still in the registers at the `iretq`.
pub fn enter_ring3() {
    interrupts::disable();
    let tid = threads::current_tid();
    let size = area_size();
    // cpu::init checked the size on every CPU before any process could start.
    assert!((cpufeat::XSAVE_LEGACY_AND_HEADER..=AREA_BYTES).contains(&size));
    // SAFETY: the calling thread owns its area while it runs and has not saved
    // into it yet (IN_RING3 is false); `size` fits both buffers.
    unsafe { core::ptr::copy_nonoverlapping(INIT.0.as_ptr(), area(tid), size) };
    // SAFETY: the area now holds the init image, a valid standard-format XSAVE area.
    unsafe { restore(area(tid)) };
    IN_RING3[tid].store(true, Ordering::Release);
}

/// The calling thread will never return to ring 3: its vector state is no longer
/// saved, so a later thread in its slot starts with none. Interrupts are off while
/// it finds its tid, so it cannot migrate halfway and clear another thread's flag.
pub fn leave_ring3() {
    interrupts::without_interrupts(|| {
        IN_RING3[threads::current_tid()].store(false, Ordering::Release);
    });
}

fn area(tid: Tid) -> *mut u8 {
    AREAS[tid].0.get().cast()
}

/// Resets the x87 component, then loads every enabled component from `area`.
///
/// # Safety
/// `area` must be a 64-byte aligned, standard-format XSAVE area that XSAVE wrote or
/// that holds the init image, and nothing may write it meanwhile.
unsafe fn restore(area: *const u8) {
    // SAFETY: INIT is a valid standard-format area (XSTATE_BV and XCOMP_BV 0, the
    // reserved header bytes 0); with only the x87 bit requested, XRSTOR initialises
    // the x87 state and leaves MXCSR alone. The caller vouches for `area`.
    unsafe {
        xrstor(INIT.0.as_ptr(), X87_ONLY);
        xrstor(area, ALL);
    }
}

/// # Safety
/// CR4.OSXSAVE must be set and `xcr0` valid for this CPU.
unsafe fn xsetbv(xcr0: u64) {
    // SAFETY: the caller's conditions are XSETBV's; it writes only XCR0.
    unsafe {
        core::arch::asm!("xsetbv", in("ecx") 0u32, in("eax") xcr0 as u32,
            in("edx") (xcr0 >> 32) as u32, options(nomem, nostack, preserves_flags));
    }
}

/// # Safety
/// `area` must be 64-byte aligned, at least the CPU's XSAVE size and not in use.
unsafe fn xsave(area: *mut u8) {
    // SAFETY: the caller's conditions are XSAVE's; it writes only the area.
    unsafe {
        core::arch::asm!("xsave64 [{}]", in(reg) area, in("eax") u32::MAX,
            in("edx") u32::MAX, options(nostack, preserves_flags));
    }
}

/// # Safety
/// `area` must be a valid, 64-byte aligned, standard-format XSAVE area.
unsafe fn xrstor(area: *const u8, mask: u64) {
    // SAFETY: the caller's conditions are XRSTOR's; it reads only the area. It
    // rewrites the x87, MXCSR and vector registers (and the x87 status and MXCSR
    // flags, so no preserves_flags), which are not declared as clobbers: the
    // soft-float kernel never allocates or reads them, as xtask's ELF check enforces.
    unsafe {
        core::arch::asm!("xrstor64 [{}]", in(reg) area, in("eax") mask as u32,
            in("edx") (mask >> 32) as u32, options(nostack, readonly));
    }
}
