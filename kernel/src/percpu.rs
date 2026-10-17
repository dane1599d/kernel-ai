//! Per-CPU data, reached through GS_BASE. Each CPU points GS at its own `PerCpu`,
//! whose first field is its own address, so `gs:[0]` finds it in one instruction.
//!
//! While a CPU runs ring-3 code its kernel GS base sits in IA32_KERNEL_GS_BASE; every
//! entry from ring 3 swaps it back with `swapgs` before any of this is used (see
//! `entry.rs`).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use x86_64::VirtAddr;
use x86_64::registers::model_specific::GsBase;

pub const MAX_CPUS: usize = 8;

#[repr(C)]
pub struct PerCpu {
    /// Must stay the first field: `this()` reads it at `gs:[0]`.
    self_ptr: AtomicU64,
    /// Top of the running thread's kernel stack; the SYSCALL entry switches to it.
    pub kernel_rsp: AtomicU64,
    /// The user stack pointer, saved by the SYSCALL entry before it switches stacks.
    pub user_rsp: AtomicU64,
    pub index: AtomicUsize,
    pub lapic_id: AtomicU32,
    pub preempt_count: AtomicU32,
    pub need_resched: AtomicBool,
    /// The thread that ran before the latest switch on this CPU.
    pub previous: AtomicUsize,
    /// The thread running now.
    pub current: AtomicUsize,
    /// When CPU time was last charged to the running thread (HPET ns).
    pub charged_at_ns: AtomicU64,
    pub ticks: AtomicU64,
    pub tick_start_ns: AtomicU64,
    /// This CPU's task state segment, for updating its ring-0 stack pointer.
    pub tss: AtomicU64,
    /// Control registers and EFER as `cpu::init` left them, for cross-CPU checks.
    pub cr0: AtomicU64,
    pub cr4: AtomicU64,
    pub efer: AtomicU64,
}

impl PerCpu {
    const fn new() -> Self {
        Self {
            self_ptr: AtomicU64::new(0),
            kernel_rsp: AtomicU64::new(0),
            user_rsp: AtomicU64::new(0),
            index: AtomicUsize::new(0),
            lapic_id: AtomicU32::new(0),
            preempt_count: AtomicU32::new(0),
            need_resched: AtomicBool::new(false),
            previous: AtomicUsize::new(0),
            current: AtomicUsize::new(0),
            charged_at_ns: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
            tick_start_ns: AtomicU64::new(0),
            tss: AtomicU64::new(0),
            cr0: AtomicU64::new(0),
            cr4: AtomicU64::new(0),
            efer: AtomicU64::new(0),
        }
    }
}

/// Offsets used by the assembly entry stubs.
pub const KERNEL_RSP_OFFSET: usize = core::mem::offset_of!(PerCpu, kernel_rsp);
pub const USER_RSP_OFFSET: usize = core::mem::offset_of!(PerCpu, user_rsp);

static CPUS: [PerCpu; MAX_CPUS] = [const { PerCpu::new() }; MAX_CPUS];
/// Set once the boot CPU has a GS base, so early code can avoid `gs:[0]`.
static READY: AtomicBool = AtomicBool::new(false);

/// Points this CPU's GS base at its `PerCpu`. Must run before anything else uses it.
pub fn init(index: usize) {
    let cpu = &CPUS[index];
    let address = cpu as *const PerCpu as u64;
    cpu.self_ptr.store(address, Ordering::Relaxed);
    cpu.index.store(index, Ordering::Relaxed);
    GsBase::write(VirtAddr::new(address));
    READY.store(true, Ordering::Release);
}

/// This CPU's data. Only valid after [`init`] ran on this CPU.
pub fn this() -> &'static PerCpu {
    let address: u64;
    // SAFETY: GS points at this CPU's PerCpu, whose first field is its own address.
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) address, options(nostack, preserves_flags, readonly));
        &*(address as *const PerCpu)
    }
}

/// This CPU's index, or 0 before per-CPU data exists (only the boot CPU runs then).
pub fn index() -> usize {
    if READY.load(Ordering::Acquire) {
        this().index.load(Ordering::Relaxed)
    } else {
        0
    }
}

pub fn get(index: usize) -> &'static PerCpu {
    &CPUS[index]
}

/// Adds `delta` to this CPU's preempt count with one GS-relative instruction.
/// Interrupts (and so migration) happen only between instructions, so the update
/// always lands on the CPU the thread is running on at that moment. Before
/// `init`, GS is not set up yet and nothing can migrate, so the update is skipped.
pub fn preempt_count_add(delta: i32) {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: GS points at this CPU's PerCpu; the field is a 32-bit atomic.
    unsafe {
        core::arch::asm!(
            "add dword ptr gs:[{offset}], {delta:e}",
            offset = const core::mem::offset_of!(PerCpu, preempt_count),
            delta = in(reg) delta,
            options(nostack)
        );
    }
}
