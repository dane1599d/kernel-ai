//! Context switching: a new thread's first frame, the stack switch itself, and
//! pointing the CPU at the next thread's address space and kernel stack.

use super::{CORES, IN_USE, ON_CPU, PINNED, SAVED_RSP, STACK_TOP, THREAD_CR3, current_tid, exit};
use crate::percpu;
use core::sync::atomic::Ordering;
use sched::Tid;
use x86_64::instructions::interrupts;

/// Builds the stack a new thread starts from: six zeroed callee-saved registers
/// (entry and argument hidden in r12 and r13) and a return into the trampoline.
pub(super) fn initial_frame(top: u64, entry: fn(usize), arg: usize) -> u64 {
    let frame = (top - 7 * 8) as *mut u64;
    let values = [
        0,
        0,
        arg as u64,
        entry as usize as u64,
        0,
        0,
        trampoline as *const () as u64,
    ];
    for (i, value) in values.iter().enumerate() {
        // SAFETY: the 56 bytes below `top` belong to the thread's stack.
        unsafe { frame.add(i).write(*value) };
    }
    frame as u64
}

/// First code a new thread runs: passes r12 and r13 to `thread_start`.
#[unsafe(naked)]
unsafe extern "C" fn trampoline() -> ! {
    core::arch::naked_asm!(
        "mov rdi, r12",
        "mov rsi, r13",
        "call {start}",
        "ud2",
        start = sym thread_start,
    )
}

extern "C" fn thread_start(entry: usize, arg: usize) -> ! {
    finish_switch();
    interrupts::enable();
    // SAFETY: `entry` was stored from a `fn(usize)` by `initial_frame`.
    let entry: fn(usize) = unsafe { core::mem::transmute(entry) };
    entry(arg);
    exit()
}

/// Saves callee-saved registers and RSP for the current thread, loads the next.
///
/// # Safety
/// Interrupts must be disabled and `load_rsp` must come from a previous save or
/// from `initial_frame`.
#[unsafe(naked)]
unsafe extern "C" fn switch_stacks(save_rsp_to: *mut u64, load_rsp: u64) {
    core::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

/// Switches this CPU from `from` to `to`. Interrupts must be off and no core locked.
pub(super) fn switch(from: Tid, to: Tid) {
    if from == to {
        return;
    }
    let cpu = percpu::this();
    cpu.previous.store(from, Ordering::Relaxed);
    ON_CPU[to].store(true, Ordering::Relaxed);
    crate::process::burst::switched_out(from);
    cpu.current.store(to, Ordering::Relaxed);
    prepare_cpu_for(to);
    // Ring-3 vector state, saved before finish_switch clears ON_CPU[from].
    crate::fpu::switch(from, to);
    let table = SAVED_RSP.0.get();
    // SAFETY: interrupts are off; `to` is runnable here and its registers are saved.
    unsafe { switch_stacks(&raw mut (*table)[from], (*table)[to]) };
    finish_switch();
}

/// Points this CPU at `to`'s address space and kernel stack before switching to it:
/// CR3 (a process's PML4, or the kernel's for kernel threads), the ring-0 stack used
/// on entry from ring 3, the SYSCALL stack, and an empty user GS base so no process
/// sees another's. Interrupts are off.
fn prepare_cpu_for(to: Tid) {
    let cr3 = match THREAD_CR3[to].load(Ordering::Relaxed) {
        0 => crate::memory::kernel_cr3(),
        cr3 => cr3,
    };
    load_cr3(cr3);
    let top = STACK_TOP[to].load(Ordering::Relaxed);
    if top != 0 {
        crate::gdt::set_ring0_stack(top);
        percpu::this().kernel_rsp.store(top, Ordering::Relaxed);
    }
    if THREAD_CR3[to].load(Ordering::Relaxed) != 0 {
        // SAFETY: IA32_KERNEL_GS_BASE holds the inactive (user) GS base while in the
        // kernel; ring 3 gets 0.
        unsafe { x86_64::registers::model_specific::Msr::new(0xC000_0102).write(0) };
    }
}

fn load_cr3(cr3: u64) {
    let current: u64;
    // SAFETY: reading CR3 has no side effects.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) current, options(nomem, nostack, preserves_flags))
    };
    if current & !0xFFF != cr3 {
        // SAFETY: `cr3` is the kernel PML4 or a process PML4 sharing its kernel half,
        // so the code and stack in use stay mapped.
        unsafe { core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags)) };
    }
}

/// Moves the calling thread onto the process page tables at `cr3`.
pub fn enter_address_space(cr3: u64) {
    interrupts::without_interrupts(|| {
        // THREAD_CR3 first: a switch after this point reloads the same CR3.
        THREAD_CR3[current_tid()].store(cr3, Ordering::Relaxed);
        load_cr3(cr3);
    });
}

/// Moves the calling thread back onto the kernel page tables, after which no CPU
/// has its old process page tables loaded.
pub fn leave_address_space() {
    interrupts::without_interrupts(|| {
        THREAD_CR3[current_tid()].store(0, Ordering::Relaxed);
        load_cr3(crate::memory::kernel_cr3());
    });
}

/// Runs on the CPU right after a switch: the previous thread's registers are now
/// saved, so it may migrate, and if it exited its slot can be reused.
fn finish_switch() {
    let previous = percpu::this().previous.load(Ordering::Relaxed);
    ON_CPU[previous].store(false, Ordering::Release);
    let me = percpu::index();
    let released = CORES[me]
        .lock()
        .as_mut()
        .is_some_and(|core| core.release(previous));
    if released {
        PINNED[previous].store(false, Ordering::Relaxed);
        IN_USE[previous].store(false, Ordering::Release);
    }
}
