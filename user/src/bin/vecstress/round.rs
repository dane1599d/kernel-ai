//! One round of vecstress in one asm block per mode, so no compiler-generated code
//! ever touches the registers under test.

use super::{Block, Mode};
use core::mem::offset_of;

/// One round in one asm block: keep the caller's MXCSR and control word, load the
/// pattern into every register (and add the last register to the others, except in
/// plain ymm mode), load MXCSR, the control word and one x87 value, spin, make the
/// system call, spin again, store everything back, then restore what was kept.
/// `$pattern` and `$out` are the offsets in `Block` the mode loads from and stores to.
macro_rules! round {
    ($block:expr, $spin:expr, $pattern:expr, $out:expr,
     [$($load:literal),*], [$($add:literal),*], [$($store:literal),*]) => {
        core::arch::asm!(
            "stmxcsr [r12 + {keep_mxcsr}]",
            "fnstcw [r12 + {keep_fcw}]",
            $($load,)*
            "ldmxcsr [r12 + {mxcsr_in}]",
            "fldcw [r12 + {fcw_in}]",
            "fild qword ptr [r12 + {x87_in}]",
            $($add,)*
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            "lea r8, [rax + r13]",
            "2:",
            "pause",
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            "cmp rax, r8",
            "jb 2b",
            "mov rax, [r12 + {call}]",
            "mov rdi, [r12 + {arg}]",
            "syscall",
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            "lea r8, [rax + r13]",
            "3:",
            "pause",
            "rdtsc",
            "shl rdx, 32",
            "or rax, rdx",
            "cmp rax, r8",
            "jb 3b",
            $($store,)*
            "stmxcsr [r12 + {mxcsr_out}]",
            "fnstcw [r12 + {fcw_out}]",
            "fistp qword ptr [r12 + {x87_out}]",
            "fnstsw word ptr [r12 + {fsw_out}]",
            "ldmxcsr [r12 + {keep_mxcsr}]",
            "fldcw [r12 + {keep_fcw}]",
            keep_mxcsr = const offset_of!(Block, keep_mxcsr),
            keep_fcw = const offset_of!(Block, keep_fcw),
            mxcsr_in = const offset_of!(Block, mxcsr_in),
            fcw_in = const offset_of!(Block, fcw_in),
            x87_in = const offset_of!(Block, x87_in),
            call = const offset_of!(Block, call),
            arg = const offset_of!(Block, arg),
            mxcsr_out = const offset_of!(Block, mxcsr_out),
            fcw_out = const offset_of!(Block, fcw_out),
            x87_out = const offset_of!(Block, x87_out),
            fsw_out = const offset_of!(Block, fsw_out),
            pattern = const $pattern,
            out = const $out,
            in("r12") $block,
            in("r13") $spin,
            clobber_abi("C"),
            options(nostack),
        )
    };
}

const PATTERN: usize = offset_of!(Block, pattern);
const OUT: usize = offset_of!(Block, out);
/// The zmm registers' pattern and what came back; k0-7 follow at +2048 (checked
/// where `ZRegs` is defined).
const ZIN: usize = offset_of!(Block, zin);
const ZOUT: usize = offset_of!(Block, zout);

/// # Safety
/// `block` must point to a live Block, and `mode` must match what the CPU and XCR0
/// allow (checked by `agrees_with_cpu`).
pub unsafe fn run_round(block: *mut Block, spin: u64, mode: Mode) {
    // SAFETY: the asm reads and writes only `*block` (through r12), keeps the x87
    // stack empty at its end, restores MXCSR and the x87 control word, and the
    // kernel preserves r12 and r13 across SYSCALL. The caller vouches for the mode.
    // Rust code never holds a value in zmm16-31 or k0-7 (the target has no
    // AVX-512), so the zmm round needs no clobbers beyond the C ABI's.
    unsafe {
        match mode {
            Mode::Zmm => round!(
                block,
                spin,
                ZIN,
                ZOUT,
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
                    "vmovdqu32 zmm\\reg, [r12 + {pattern} + 64*\\reg]",
                    ".endr",
                    ".irp reg, 0,1,2,3,4,5,6,7",
                    "kmovq k\\reg, [r12 + {pattern} + 2048 + 8*\\reg]",
                    ".endr"
                ],
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30",
                    "vpaddd zmm\\reg, zmm\\reg, zmm31",
                    ".endr"
                ],
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
                    "vmovdqu32 [r12 + {out} + 64*\\reg], zmm\\reg",
                    ".endr",
                    ".irp reg, 0,1,2,3,4,5,6,7",
                    "kmovq [r12 + {out} + 2048 + 8*\\reg], k\\reg",
                    ".endr",
                    "vzeroupper"
                ]
            ),
            Mode::Avx2 => round!(
                block,
                spin,
                PATTERN,
                OUT,
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
                    "vmovdqu ymm\\reg, [r12 + {pattern} + 32*\\reg]",
                    ".endr"
                ],
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14",
                    "vpaddd ymm\\reg, ymm\\reg, ymm15",
                    ".endr"
                ],
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
                    "vmovdqu [r12 + {out} + 32*\\reg], ymm\\reg",
                    ".endr",
                    "vzeroupper"
                ]
            ),
            Mode::Ymm => round!(
                block,
                spin,
                PATTERN,
                OUT,
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
                    "vmovdqu ymm\\reg, [r12 + {pattern} + 32*\\reg]",
                    ".endr"
                ],
                [],
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
                    "vmovdqu [r12 + {out} + 32*\\reg], ymm\\reg",
                    ".endr",
                    "vzeroupper"
                ]
            ),
            Mode::Xmm => round!(
                block,
                spin,
                PATTERN,
                OUT,
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
                    "movdqu xmm\\reg, [r12 + {pattern} + 32*\\reg]",
                    ".endr"
                ],
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14",
                    "paddd xmm\\reg, xmm15",
                    ".endr"
                ],
                [
                    ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
                    "movdqu [r12 + {out} + 32*\\reg], xmm\\reg",
                    ".endr"
                ]
            ),
        }
    }
}
