//! Fault reporting for the kernel: the fault screen, plus deliberate faults for the
//! M1 and M7 exit tests. Faults in ring 3 never come here; they kill the process
//! (see `process::user_fault`).

use crate::backtrace;
use crate::entry::TrapFrame;
use crate::lines::{self, Describe};
use crate::log;
use crate::qemu::ExitCode;
use core::fmt;
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use x86_64::registers::control::Cr2;

/// Unmapped lower-half address used by the page-fault test.
pub const PAGE_FAULT_TARGET: u64 = 0x0000_0DEA_D000_0000;
/// Unmapped stack pointer used by the double-fault test.
const BAD_STACK: u64 = 0x0000_0DEA_D000_1000;

/// Page-fault error code bits.
pub mod pf {
    pub const PRESENT: u64 = 1 << 0;
    pub const WRITE: u64 = 1 << 1;
    pub const FETCH: u64 = 1 << 4;
}

/// A CPU exception, from its vector, error code and (for page faults) CR2.
pub enum Exception {
    PageFault { address: u64, code: u64 },
    GeneralProtection { code: u64 },
    DoubleFault,
    Other { vector: u64, code: u64 },
}

impl Exception {
    pub fn from_frame(frame: &TrapFrame) -> Self {
        match frame.vector {
            14 => Self::PageFault {
                address: Cr2::read_raw(),
                code: frame.error_code,
            },
            13 => Self::GeneralProtection {
                code: frame.error_code,
            },
            8 => Self::DoubleFault,
            vector => Self::Other {
                vector,
                code: frame.error_code,
            },
        }
    }
}

pub fn vector_name(vector: u64) -> &'static str {
    match vector {
        0 => "DIVIDE ERROR",
        1 => "DEBUG",
        2 => "NMI",
        3 => "BREAKPOINT",
        4 => "OVERFLOW",
        5 => "BOUND RANGE",
        6 => "INVALID OPCODE",
        7 => "DEVICE NOT AVAILABLE",
        8 => "DOUBLE FAULT",
        10 => "INVALID TSS",
        11 => "SEGMENT NOT PRESENT",
        12 => "STACK SEGMENT FAULT",
        13 => "GENERAL PROTECTION",
        14 => "PAGE FAULT",
        16 => "X87 FLOATING POINT",
        17 => "ALIGNMENT CHECK",
        18 => "MACHINE CHECK",
        19 => "SIMD FLOATING POINT",
        21 => "CONTROL PROTECTION",
        _ => "EXCEPTION",
    }
}

impl fmt::Display for Exception {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PageFault { address, code } => {
                let access = if code & pf::FETCH != 0 {
                    "fetch"
                } else if code & pf::WRITE != 0 {
                    "write"
                } else {
                    "read"
                };
                let why = if code & pf::PRESENT != 0 {
                    "protection"
                } else {
                    "not present"
                };
                write!(
                    f,
                    "PAGE FAULT at {address:#018x} ({access}, {why}, code {code:#x})"
                )
            }
            Self::GeneralProtection { code } => write!(f, "GENERAL PROTECTION (code {code:#x})"),
            Self::DoubleFault => f.write_str("DOUBLE FAULT"),
            Self::Other { vector, code } => {
                write!(
                    f,
                    "{} (vector {vector}, code {code:#x})",
                    vector_name(*vector)
                )
            }
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
/// The address a write-protect test writes to, or the guard page a stack-overflow
/// test hits.
static EXPECTED_ADDRESS: AtomicU64 = AtomicU64::new(0);
const PAGE: u64 = 4096;

fn expect(kind: Expected, address: u64) {
    EXPECTED_ADDRESS.store(address, Ordering::SeqCst);
    EXPECTED.store(kind as u8, Ordering::SeqCst);
}

/// The next fault must be a write to the read-only page at `address`.
pub fn expect_write_fault(address: u64) {
    expect(Expected::WriteProtect, address);
}

/// The next fault must be a double fault caused by running into the guard page at `guard`.
pub fn expect_stack_overflow(guard: u64) {
    expect(Expected::StackOverflow, guard);
}

static BREAKPOINTS: AtomicU32 = AtomicU32::new(0);

pub fn breakpoint_count() -> u32 {
    BREAKPOINTS.load(Ordering::SeqCst)
}

/// Breakpoints are reported and then resumed.
pub fn breakpoint(frame: &TrapFrame) {
    BREAKPOINTS.fetch_add(1, Ordering::SeqCst);
    // int3 leaves RIP after the one-byte instruction.
    log!("breakpoint at {} (resuming)", Describe(frame.rip - 1));
}

/// Shows the fault screen. In test mode, exits QEMU with pass if this fault was
/// the expected one and its report is correct, and fail otherwise.
pub fn fatal(frame: &TrapFrame) -> ! {
    let fault = Exception::from_frame(frame);
    crate::smp::begin_fatal_report();
    log!("");
    log!(
        "=== CPU EXCEPTION on cpu {}: {fault} ===",
        crate::percpu::index()
    );
    log!("rip: {}", Describe(frame.rip));
    log!("rsp: {:#018x}", frame.rsp);
    log!("backtrace:");
    backtrace::print_from(frame.rbp);
    match verdict(&fault, frame) {
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

fn verdict(fault: &Exception, frame: &TrapFrame) -> Result<(), &'static str> {
    let expected = match EXPECTED.load(Ordering::SeqCst) {
        1 => Expected::PageFault,
        2 => Expected::DoubleFault,
        3 => Expected::WriteProtect,
        4 => Expected::StackOverflow,
        _ => Expected::Nothing,
    };
    let target = EXPECTED_ADDRESS.load(Ordering::SeqCst);
    let rip = frame.rip;
    match (expected, fault) {
        (Expected::WriteProtect, Exception::PageFault { address, code }) => {
            if *address != target {
                Err("write-protect fault reported the wrong address")
            } else if code & (pf::PRESENT | pf::WRITE) != pf::PRESENT | pf::WRITE {
                Err("write-protect fault was not a write to a present page")
            } else {
                Ok(())
            }
        }
        (Expected::StackOverflow, Exception::DoubleFault) => {
            if (target..target + 2 * PAGE).contains(&frame.rsp) {
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
