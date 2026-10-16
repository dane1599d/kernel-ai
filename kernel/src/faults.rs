//! Fault reporting: the fault screen, plus deliberate faults for the M1 exit test.

use crate::backtrace;
use crate::lines::{self, Describe};
use crate::log;
use crate::qemu::ExitCode;
use core::fmt;
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use x86_64::structures::idt::{InterruptStackFrame, PageFaultErrorCode};

/// Unmapped lower-half address used by the page-fault test.
pub const PAGE_FAULT_TARGET: u64 = 0x0000_0DEA_D000_0000;
/// Unmapped stack pointer used by the double-fault test.
const BAD_STACK: u64 = 0x0000_0DEA_D000_1000;

pub enum Exception {
    DivideError,
    InvalidOpcode,
    GeneralProtection {
        code: u64,
    },
    PageFault {
        address: u64,
        code: PageFaultErrorCode,
    },
    DoubleFault,
}

impl fmt::Display for Exception {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DivideError => f.write_str("DIVIDE ERROR"),
            Self::InvalidOpcode => f.write_str("INVALID OPCODE"),
            Self::GeneralProtection { code } => write!(f, "GENERAL PROTECTION (code {code:#x})"),
            Self::PageFault { address, code } => {
                write!(f, "PAGE FAULT at {address:#018x} ({code:?})")
            }
            Self::DoubleFault => f.write_str("DOUBLE FAULT"),
        }
    }
}

/// Which fault a self-test is about to cause on purpose.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Expected {
    Nothing = 0,
    PageFault = 1,
    DoubleFault = 2,
    WriteProtect = 3,
    StackOverflow = 4,
}

static EXPECTED: AtomicU8 = AtomicU8::new(Expected::Nothing as u8);
/// The address a write-protect test writes to, or the guard page a stack-overflow test hits.
static EXPECTED_ADDRESS: AtomicU64 = AtomicU64::new(0);
const PAGE: u64 = 4096;

/// The next fault must be a write to the read-only page at `address`.
pub fn expect_write_fault(address: u64) {
    EXPECTED_ADDRESS.store(address, Ordering::SeqCst);
    EXPECTED.store(Expected::WriteProtect as u8, Ordering::SeqCst);
}

/// The next fault must be a double fault caused by running into the guard page at `guard`.
pub fn expect_stack_overflow(guard: u64) {
    EXPECTED_ADDRESS.store(guard, Ordering::SeqCst);
    EXPECTED.store(Expected::StackOverflow as u8, Ordering::SeqCst);
}
static BREAKPOINTS: AtomicU32 = AtomicU32::new(0);

pub fn breakpoint_count() -> u32 {
    BREAKPOINTS.load(Ordering::SeqCst)
}

/// Breakpoints are reported and then resumed.
pub fn breakpoint(frame: &InterruptStackFrame) {
    BREAKPOINTS.fetch_add(1, Ordering::SeqCst);
    // int3 leaves RIP after the one-byte instruction.
    let rip = frame.instruction_pointer.as_u64() - 1;
    log!("breakpoint at {} (resuming)", Describe(rip));
}

/// Shows the fault screen. In test mode, exits QEMU with pass if this fault was
/// the expected one and its report is correct, and fail otherwise.
pub fn fatal(fault: Exception, frame: &InterruptStackFrame, handler_rbp: u64) -> ! {
    let rip = frame.instruction_pointer.as_u64();
    log!("");
    log!("=== CPU EXCEPTION: {fault} ===");
    log!("rip: {}", Describe(rip));
    log!("rsp: {:#018x}", frame.stack_pointer.as_u64());
    log!("backtrace:");
    // SAFETY: the handler's own frame begins with the interrupted code's saved RBP.
    let interrupted_rbp = unsafe { (handler_rbp as *const u64).read() };
    backtrace::print_from(interrupted_rbp);

    let expected = match EXPECTED.load(Ordering::SeqCst) {
        1 => Expected::PageFault,
        2 => Expected::DoubleFault,
        3 => Expected::WriteProtect,
        4 => Expected::StackOverflow,
        _ => Expected::Nothing,
    };
    let target = EXPECTED_ADDRESS.load(Ordering::SeqCst);
    let rsp = frame.stack_pointer.as_u64();
    let verdict = match (expected, &fault) {
        (Expected::WriteProtect, Exception::PageFault { address, code }) => {
            let write_to_present =
                PageFaultErrorCode::PROTECTION_VIOLATION | PageFaultErrorCode::CAUSED_BY_WRITE;
            if *address != target {
                Err("write-protect fault reported the wrong address")
            } else if !code.contains(write_to_present) {
                Err("write-protect fault was not a write to a present page")
            } else {
                Ok(())
            }
        }
        (Expected::StackOverflow, Exception::DoubleFault) => {
            if (target..target + 2 * PAGE).contains(&rsp) {
                Ok(())
            } else {
                Err("double fault did not come from the guard page")
            }
        }
        (Expected::PageFault, Exception::PageFault { address, .. }) => {
            if *address != PAGE_FAULT_TARGET {
                Err("page fault reported the wrong address")
            } else if !in_file(rip, "kernel/src/faults.rs") {
                Err("page fault RIP did not resolve to kernel/src/faults.rs")
            } else {
                Ok(())
            }
        }
        (Expected::DoubleFault, Exception::DoubleFault) => Ok(()),
        _ => Err("unexpected CPU exception"),
    };
    match verdict {
        Ok(()) => {
            log!("self-test: expected fault handled correctly");
            crate::test_exit(ExitCode::Success)
        }
        Err(reason) => {
            log!("self-test: {reason}");
            crate::test_exit(ExitCode::Failure)
        }
    }
}

fn in_file(addr: u64, file: &str) -> bool {
    lines::resolve(addr)
        .and_then(|location| location.file)
        .is_some_and(|found| found.ends_with(file))
}

/// Reads an unmapped address. The load is inline asm so the faulting RIP is in this file.
#[inline(never)]
pub fn trigger_page_fault() {
    EXPECTED.store(Expected::PageFault as u8, Ordering::SeqCst);
    // SAFETY: deliberately faults; the page-fault handler never returns here.
    unsafe {
        core::arch::asm!("mov {tmp}, [{addr}]", addr = in(reg) PAGE_FAULT_TARGET, tmp = out(reg) _);
    }
}

/// Points RSP at unmapped memory and pushes. The CPU cannot push the page-fault frame
/// either, so it raises a double fault, which must run on the IST stack.
pub fn trigger_double_fault() -> ! {
    EXPECTED.store(Expected::DoubleFault as u8, Ordering::SeqCst);
    // SAFETY: deliberately faults; the double-fault handler never returns.
    unsafe { core::arch::asm!("mov rsp, {0}", "push rax", in(reg) BAD_STACK, options(noreturn)) }
}

/// Exposed for the boot self-test so it can check that the table resolves.
pub fn self_check_address() -> u64 {
    trigger_page_fault as *const () as u64
}
