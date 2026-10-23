//! The interrupt descriptor table and the Rust side of every interrupt and exception.
//!
//! All gates are 64-bit interrupt gates with DPL 0 (ring 3 cannot raise any vector
//! with `int n`) pointing at the stubs in `entry.rs`. NMI, double fault and machine
//! check run on their own IST stacks.

use crate::entry::{self, TrapFrame};
use crate::gdt::{DOUBLE_FAULT_IST_INDEX, KERNEL_CODE, MACHINE_CHECK_IST_INDEX, NMI_IST_INDEX};
use crate::{apic, cpu, faults, i8042, process, smp, threads, timer, usb};
use spin::LazyLock;
use x86_64::VirtAddr;
use x86_64::instructions::tables::lidt;
use x86_64::structures::DescriptorTablePointer;

#[derive(Clone, Copy)]
#[repr(C)]
struct Gate {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl Gate {
    const MISSING: Gate = Gate {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    /// A present 64-bit interrupt gate, DPL 0. `ist` is a TSS IST index, or None.
    fn new(handler: unsafe extern "C" fn(), ist: Option<u16>) -> Gate {
        let address = handler as usize as u64;
        Gate {
            offset_low: address as u16,
            selector: KERNEL_CODE,
            ist: ist.map_or(0, |index| index as u8 + 1),
            attributes: 0x8E,
            offset_mid: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, align(16))]
struct Idt([Gate; 256]);

static IDT: LazyLock<Idt> = LazyLock::new(|| {
    let mut gates = [Gate::MISSING; 256];
    for (vector, stub) in entry::EXCEPTIONS.iter().enumerate() {
        let ist = match vector {
            2 => Some(NMI_IST_INDEX),
            8 => Some(DOUBLE_FAULT_IST_INDEX),
            18 => Some(MACHINE_CHECK_IST_INDEX),
            _ => None,
        };
        gates[vector] = Gate::new(*stub, ist);
    }
    let devices: [(u8, unsafe extern "C" fn()); 7] = [
        (apic::TIMER_VECTOR, entry::vector_timer),
        (apic::KEYBOARD_VECTOR, entry::vector_keyboard),
        (apic::RESCHEDULE_VECTOR, entry::vector_reschedule),
        (apic::TLB_VECTOR, entry::vector_tlb),
        (apic::HALT_VECTOR, entry::vector_halt),
        (apic::SPURIOUS_VECTOR, entry::vector_spurious),
        (apic::MOUSE_VECTOR, entry::vector_mouse),
    ];
    for (vector, stub) in devices {
        gates[usize::from(vector)] = Gate::new(stub, None);
    }
    Idt(gates)
});

pub fn init() {
    let pointer = DescriptorTablePointer {
        limit: (core::mem::size_of::<Idt>() - 1) as u16,
        base: VirtAddr::new(&*IDT as *const Idt as u64),
    };
    // SAFETY: the table is 'static and every present gate points at a valid stub.
    unsafe { lidt(&pointer) };
}

/// Called by `entry::common_entry` for every non-IST vector, with interrupts off.
pub extern "C" fn trap_dispatch(frame: &mut TrapFrame) {
    cpu::clac();
    match frame.vector {
        v if v == u64::from(apic::TIMER_VECTOR) => {
            timer::on_tick();
            process::burst::on_tick();
            let waiter = usb::on_tick();
            apic::end_of_interrupt();
            if let Some(tid) = waiter {
                threads::wake(tid);
            }
            threads::on_tick(frame.rip, frame.rbp);
        }
        // Both PS/2 ports: either interrupt drains whatever the controller holds.
        v if v == u64::from(apic::KEYBOARD_VECTOR) || v == u64::from(apic::MOUSE_VECTOR) => {
            let waiter = i8042::on_interrupt();
            apic::end_of_interrupt();
            if let Some(tid) = waiter {
                threads::wake(tid);
            }
        }
        // Spurious interrupts must not be acknowledged.
        v if v == u64::from(apic::SPURIOUS_VECTOR) => {}
        v if v == u64::from(apic::RESCHEDULE_VECTOR) => {
            apic::end_of_interrupt();
            threads::on_reschedule_ipi();
        }
        v if v == u64::from(apic::TLB_VECTOR) => {
            smp::on_tlb_ipi();
            apic::end_of_interrupt();
        }
        // Another CPU hit a fatal error: stop here so its report is not interleaved.
        v if v == u64::from(apic::HALT_VECTOR) => crate::halt_forever(),
        0..=31 => exception(frame),
        _ => faults::fatal(frame),
    }
    if frame.user_mode() {
        process::return_to_user(frame);
    }
}

fn exception(frame: &mut TrapFrame) {
    if frame.user_mode() || frame.is_user_iret_fault() {
        // Ring 3 faulted (or its return did): the process dies, the kernel does not.
        process::user_fault(frame);
    }
    match frame.vector {
        // Breakpoints in the kernel are reported and resumed.
        3 => faults::breakpoint(frame),
        _ => faults::fatal(frame),
    }
}

/// Called by `entry::paranoid_entry` for NMI, double fault and machine check.
pub extern "C" fn fatal_dispatch(frame: &mut TrapFrame) -> ! {
    faults::fatal(frame)
}
