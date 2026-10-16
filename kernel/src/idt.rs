//! Interrupt descriptor table and CPU exception handlers.

use crate::backtrace::current_rbp;
use crate::faults::{self, Exception};
use crate::gdt::DOUBLE_FAULT_IST_INDEX;
use crate::{apic, keyboard, timer};
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
}

extern "x86-interrupt" fn keyboard_interrupt(_frame: InterruptStackFrame) {
    keyboard::on_interrupt();
    apic::end_of_interrupt();
}

/// Spurious interrupts must not be acknowledged.
extern "x86-interrupt" fn spurious_interrupt(_frame: InterruptStackFrame) {}
