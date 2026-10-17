//! Test service: on its Nth start tries operation `N % OPS.len()` from
//! `kabi::probe::OPS`, each of which ring 3 must not be allowed to do. The kernel
//! kills it with the exception that stopped it; the supervisor starts it again, so
//! it works through the whole list.

#![no_std]
#![no_main]

use kabi::probe::{OPS, SGDT};
use user::{arg, args, println, sys};

user::entry!(main);

/// A canonical kernel-half address (the start of the direct map).
const KERNEL_ADDRESS: u64 = 0xFFFF_8000_0000_0000;

static DATA: [u8; 16] = [0xC3; 16];

fn main() -> i32 {
    let args = args();
    let restart: usize = arg(&args, "restart")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let op = restart % OPS.len();
    println!("op {op}: {}", OPS[op].0);
    // SAFETY: every operation below is meant to fault and end this process.
    unsafe { attempt(op) };
    if op == SGDT {
        println!("sgdt allowed (UMIP is off)");
        return 0;
    }
    println!("op {op} was not stopped");
    1
}

#[inline(never)]
unsafe fn attempt(op: usize) {
    // SAFETY: deliberate privilege violations; the kernel kills the process.
    unsafe {
        match op {
            0 => core::arch::asm!("fld1", options(nostack)),
            1 => core::arch::asm!(".byte 0x0f, 0x57, 0xc0", options(nostack)), // xorps xmm0, xmm0
            2 => core::arch::asm!(".byte 0xc5, 0xf8, 0x57, 0xc0", options(nostack)), // vxorps xmm0, xmm0, xmm0
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
            _ => sys::yield_now(),
        }
    }
}
