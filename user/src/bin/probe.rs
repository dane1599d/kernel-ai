//! Test service: on its Nth start tries operation `N % OPS.len()` from
//! `kabi::probe::OPS`. Ops 0-2 (x87, SSE and AVX) must complete with the right
//! answer, and the probe exits 0. Ring 3 must not be allowed to do any other op: the
//! kernel kills the probe with the exception that stopped it, and its supervisor (or
//! the kernel's `probe` case) starts it again, so it works through the whole list.

#![no_std]
#![no_main]

use kabi::probe::{OPS, SGDT};
use user::{arg, args, println, sys};

user::entry!(main);

/// A canonical kernel-half address (the start of the direct map).
const KERNEL_ADDRESS: u64 = 0xFFFF_8000_0000_0000;
/// What ops 0-2 compute: 1 + 1 on the x87 unit, and the sign masks of four and
/// eight all-ones lanes from SSE and AVX compares.
const ANSWERS: [u64; 3] = [2, 0xF, 0xFF];
/// An x87 control word with only the divide-by-zero exception unmasked.
const FCW_ZERO_DIVIDE_UNMASKED: u16 = 0x037B;

static DATA: [u8; 16] = [0xC3; 16];

fn main() -> i32 {
    let args = args();
    let restart: usize = arg(&args, "restart")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let op = restart % OPS.len();
    println!("op {op}: {}", OPS[op].0);
    // SAFETY: ops 0-2 only compute; every other op is meant to fault and end this
    // process.
    let answer = unsafe { attempt(op) };
    if OPS[op].1.is_empty() {
        if answer == ANSWERS[op] {
            println!("op {op} completed");
            return 0;
        }
        println!("op {op} gave {answer:#x}, not {:#x}", ANSWERS[op]);
        return 2;
    }
    if op == SGDT {
        println!("sgdt allowed (UMIP is off)");
        return 0;
    }
    println!("op {op} was not stopped");
    1
}

/// Tries op `op`; for ops 0-2, returns what they computed.
#[inline(never)]
unsafe fn attempt(op: usize) -> u64 {
    let mut answer = 0u64;
    // SAFETY: ops 0-2 write only `answer` (op 0 through memory, which no
    // instruction of it can redirect; ops 1 and 2 in RAX, an explicit output) and
    // leave the x87 stack empty; the others are deliberate privilege violations,
    // and the kernel kills the process.
    unsafe {
        match op {
            0 => core::arch::asm!(
                "fld1",
                "fld1",
                "faddp st(1), st",
                "fistp qword ptr [{out}]",
                out = in(reg) &raw mut answer,
                clobber_abi("C"),
                options(nostack),
            ),
            1 => core::arch::asm!(
                "pcmpeqd xmm0, xmm0",
                "movmskps eax, xmm0",
                out("rax") answer,
                clobber_abi("C"),
                options(nomem, nostack),
            ),
            2 => core::arch::asm!(
                "vxorps ymm0, ymm0, ymm0",
                "vcmpeqps ymm1, ymm0, ymm0",
                "vmovmskps eax, ymm1",
                "vzeroupper",
                out("rax") answer,
                clobber_abi("C"),
                options(nomem, nostack),
            ),
            3 => core::arch::asm!("cli", options(nostack)),
            4 => core::arch::asm!("hlt", options(nostack)),
            5 => core::arch::asm!("in al, 0x60", out("al") _, options(nostack)),
            6 => {
                core::arch::asm!("rdmsr", in("ecx") 0xC000_0080u32, out("eax") _, out("edx") _, options(nostack))
            }
            7 => core::arch::asm!("mov cr3, {}", in(reg) 0u64, options(nostack)),
            8 => {
                let mut gdtr = [0u8; 10];
                core::arch::asm!("sgdt [{}]", in(reg) gdtr.as_mut_ptr(), options(nostack));
            }
            9 => core::arch::asm!("swapgs", options(nostack)),
            10 => core::arch::asm!("int 0x80", options(nostack)),
            11 => core::arch::asm!("int3", options(nostack)),
            12 => core::arch::asm!("sysenter", options(nostack)),
            13 => {
                let code = attempt as *const () as *mut u8;
                core::ptr::write_volatile(code, 0xCC);
            }
            14 => {
                let data: extern "C" fn() = core::mem::transmute(DATA.as_ptr());
                data();
            }
            15 => {
                core::ptr::read_volatile(KERNEL_ADDRESS as *const u64);
            }
            16 => core::arch::asm!(
                "mov r12, rsp",
                "mov rsp, {kernel}",
                "syscall",
                "mov rsp, r12",
                kernel = in(reg) KERNEL_ADDRESS + 0x1000,
                inout("rax") kabi::call::TIME => _,
                out("r12") _,
                out("rcx") _,
                out("r11") _,
            ),
            // 1 / 0 with the divide-by-zero exception unmasked: the error is pending
            // after fidiv, and fwait raises it.
            17 => core::arch::asm!(
                "fninit",
                "fldcw [{cw}]",
                "fld1",
                "fidiv dword ptr [{zero}]",
                "fwait",
                "fstp st(0)",
                "fninit",
                cw = in(reg) &FCW_ZERO_DIVIDE_UNMASKED,
                zero = in(reg) &0u32,
                clobber_abi("C"),
                options(nostack),
            ),
            _ => sys::yield_now(),
        }
    }
    answer
}
