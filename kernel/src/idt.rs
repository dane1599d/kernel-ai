//! Interrupt descriptor table and CPU exception handlers.

use crate::backtrace::current_rbp;
use crate::faults::{self, Exception};
use crate::gdt::DOUBLE_FAULT_IST_INDEX;
use crate::{apic, keyboard, smp, threads, timer};
use spin::LazyLock;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

static IDT: LazyLock<InterruptDescriptorTable> = LazyLock::new(|| {
    let mut idt = InterruptDescriptorTable::new();
    idt.divide_error.set_handler_fn(divide_error);
    idt.breakpoint.set_handler_fn(breakpoint);
    idt.invalid_opcode.set_handler_fn(invalid_opcode);
    idt.general_protection_fault
        .set_handler_fn(general_protection_fault);
    idt.page_fault.set_handler_fn(page_fault);
    // SAFETY: the IST index refers to a stack the TSS in gdt.rs sets up.
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault)
            .set_stack_index(DOUBLE_FAULT_IST_INDEX);
    }
    idt[apic::TIMER_VECTOR].set_handler_fn(timer_interrupt);
    idt[apic::KEYBOARD_VECTOR].set_handler_fn(keyboard_interrupt);
    idt[apic::SPURIOUS_VECTOR].set_handler_fn(spurious_interrupt);
    idt[apic::RESCHEDULE_VECTOR].set_handler_fn(reschedule_ipi);
    idt[apic::TLB_VECTOR].set_handler_fn(tlb_ipi);
    idt[apic::HALT_VECTOR].set_handler_fn(halt_ipi);
    idt
});

pub fn init() {
    IDT.load();
}

extern "x86-interrupt" fn breakpoint(frame: InterruptStackFrame) {
    faults::breakpoint(&frame);
}

extern "x86-interrupt" fn divide_error(frame: InterruptStackFrame) {
    faults::fatal(Exception::DivideError, &frame, current_rbp());
}

extern "x86-interrupt" fn invalid_opcode(frame: InterruptStackFrame) {
    faults::fatal(Exception::InvalidOpcode, &frame, current_rbp());
}

extern "x86-interrupt" fn general_protection_fault(frame: InterruptStackFrame, code: u64) {
    faults::fatal(Exception::GeneralProtection { code }, &frame, current_rbp());
}

extern "x86-interrupt" fn page_fault(frame: InterruptStackFrame, code: PageFaultErrorCode) {
    let address = Cr2::read_raw();
    faults::fatal(
        Exception::PageFault { address, code },
        &frame,
        current_rbp(),
    );
}

extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, _code: u64) -> ! {
    faults::fatal(Exception::DoubleFault, &frame, current_rbp());
}

extern "x86-interrupt" fn timer_interrupt(_frame: InterruptStackFrame) {
    timer::on_tick();
    apic::end_of_interrupt();
    threads::on_tick();
}

extern "x86-interrupt" fn keyboard_interrupt(_frame: InterruptStackFrame) {
    let waiter = keyboard::on_interrupt();
    apic::end_of_interrupt();
    if let Some(tid) = waiter {
        threads::wake(tid);
    }
}

/// Spurious interrupts must not be acknowledged.
extern "x86-interrupt" fn spurious_interrupt(_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn reschedule_ipi(_frame: InterruptStackFrame) {
    apic::end_of_interrupt();
    threads::on_reschedule_ipi();
}

extern "x86-interrupt" fn tlb_ipi(_frame: InterruptStackFrame) {
    smp::on_tlb_ipi();
    apic::end_of_interrupt();
}

/// Another CPU hit a fatal error: stop here so its report is not interleaved.
extern "x86-interrupt" fn halt_ipi(_frame: InterruptStackFrame) {
    crate::halt_forever();
}
