//! Test program for the kernel's `fpu` case. It first checks that it started with
//! clean vector registers, then for many rounds loads a pattern of its own into all
//! sixteen vector registers, MXCSR and the x87 unit, spins, makes a system call,
//! spins again and checks every bit survived preemption, the call and any migration
//! to another CPU. Everything between loading and checking is one asm block, so no
//! compiler-generated code ever touches the registers under test.
//!
//! Arguments: `seed=N` picks the pattern (each process gets its own), `rounds=N`,
//! `spin=N` TSC cycles per spin, and `ymm=1` and `avx2=1` say what the kernel found,
//! which the program checks against CPUID itself. With ymm it uses all 256 bits of
//! every register; with avx2 it also adds the registers with VPADDD, and without
//! ymm it adds the xmm registers with PADDD.
//!
//! Exit codes: 0 every round kept its state, 1 bad arguments, 2 dirty at start,
//! 3 state lost, 4 the CPU disagrees with the kernel's arguments.

#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::mem::offset_of;
use user::{arg, args, println, sys};

const MXCSR_INIT: u32 = 0x1F80;
const FCW_INIT: u16 = 0x037F;
/// FSW bits that must be clear after a round: the exception flags, the stack
/// fault and summary bits, and TOP (the x87 stack is empty again).
const FSW_CHECKED: u16 = 0x38FF;
const YMM_LANES: usize = 8;
const XMM_LANES: usize = 4;
const REGS: usize = 16;

/// What `_start` saved before any Rust code ran: FXSAVE64's 512 bytes (x87, MXCSR
/// and xmm0-15), the upper halves of ymm0-15, and whether it could read them.
#[repr(C, align(64))]
struct Start {
    fx: [u8; 512],
    upper: [[u8; 16]; REGS],
    has_upper: u8,
}

#[repr(transparent)]
struct StartCell(UnsafeCell<Start>);

// SAFETY: this process has one thread; `_start` writes the cell before `main` runs,
// and `main` only reads it.
unsafe impl Sync for StartCell {}

static START: StartCell = StartCell(UnsafeCell::new(Start {
    fx: [0; 512],
    upper: [[0; 16]; REGS],
    has_upper: 0,
}));

/// Saves the vector state the kernel handed over, then runs `main`. GPRs are all zero
/// on entry, so CPUID may clobber RBX.
///
/// # Safety
/// Only the kernel calls it, once, as the program's entry point.
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "lea rdi, [rip + {start}]",
        "fxsave64 [rdi]",
        // The upper ymm halves exist only if CPUID has OSXSAVE and AVX, and XCR0
        // has SSE and AVX.
        "mov eax, 1",
        "cpuid",
        "and ecx, 0x18000000",
        "cmp ecx, 0x18000000",
        "jne 2f",
        "xor ecx, ecx",
        "xgetbv",
        "and eax, 6",
        "cmp eax, 6",
        "jne 2f",
        ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
        "vextractf128 [rdi + {upper} + 16*\\reg], ymm\\reg, 1",
        ".endr",
        "mov byte ptr [rdi + {has_upper}], 1",
        "2:",
        "and rsp, -16",
        "call {run}",
        "ud2",
        start = sym START,
        upper = const offset_of!(Start, upper),
        has_upper = const offset_of!(Start, has_upper),
        run = sym run,
    )
}

extern "C" fn run() -> ! {
    sys::exit(main())
}

/// One round's pattern, what came back, and the control values, at offsets the asm
/// blocks name.
#[repr(C, align(64))]
struct Block {
    pattern: [[u32; YMM_LANES]; REGS],
    out: [[u32; YMM_LANES]; REGS],
    x87_in: i64,
    x87_out: i64,
    call: u64,
    arg: u64,
    mxcsr_in: u32,
    mxcsr_out: u32,
    keep_mxcsr: u32,
    fcw_in: u16,
    fcw_out: u16,
    fsw_out: u16,
    keep_fcw: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Xmm,
    Ymm,
    Avx2,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Xmm => "xmm",
            Mode::Ymm => "ymm",
            Mode::Avx2 => "ymm with avx2",
        }
    }
}

fn main() -> i32 {
    let args = args();
    let number = |key: &str| arg(&args, key).and_then(|v| v.parse::<u64>().ok());
    let (Some(seed), Some(rounds), Some(spin), Some(ymm), Some(avx2)) = (
        number("seed"),
        number("rounds"),
        number("spin"),
        number("ymm"),
        number("avx2"),
    ) else {
        println!("vecstress: needs seed=, rounds=, spin=, ymm= and avx2=");
        return 1;
    };
    let mode = match (ymm, avx2) {
        (0, 0) => Mode::Xmm,
        (1, 0) => Mode::Ymm,
        (1, 1) => Mode::Avx2,
        _ => {
            println!("vecstress {seed}: avx2=1 needs ymm=1");
            return 1;
        }
    };
    // SAFETY: `_start` finished writing START before `main` ran.
    let start = unsafe { &*START.0.get() };
    if let Err(code) = agrees_with_cpu(seed, mode, start) {
        return code;
    }
    if let Some(dirty) = dirty_start(start) {
        println!("vecstress {seed}: dirty at start: {dirty}");
        return 2;
    }
    let mut block = Block {
        pattern: [[0; YMM_LANES]; REGS],
        out: [[0; YMM_LANES]; REGS],
        x87_in: 0,
        x87_out: 0,
        call: 0,
        arg: 0,
        mxcsr_in: 0,
        mxcsr_out: 0,
        keep_mxcsr: 0,
        fcw_in: 0,
        fcw_out: 0,
        fsw_out: 0,
        keep_fcw: 0,
    };
    for round in 0..rounds {
        prepare(&mut block, seed, round);
        // SAFETY: `block` is a live, aligned Block; the asm block writes only its
        // `out`, `*_out` and `keep_*` fields and restores MXCSR and the control word.
        unsafe { run_round(&mut block, spin, mode) };
        if let Some(lost) = lost(&block, mode) {
            println!("vecstress {seed}: round {round}: {lost}");
            return 3;
        }
    }
    println!(
        "vecstress {seed}: {rounds} rounds kept their {} state",
        mode.name()
    );
    0
}

/// The kernel's arguments must say what CPUID says, and `_start` must have read
/// the upper ymm halves exactly when ymm state is on.
fn agrees_with_cpu(seed: u64, mode: Mode, start: &Start) -> Result<(), i32> {
    let cpu = cpufeat::Features::detect();
    let ymm = cpu.avx && cpu.ymm_enabled();
    let found = match (ymm, cpu.avx2 && ymm) {
        (false, _) => Mode::Xmm,
        (true, false) => Mode::Ymm,
        (true, true) => Mode::Avx2,
    };
    if found != mode {
        println!(
            "vecstress {seed}: the kernel said {}, CPUID says {}",
            mode.name(),
            found.name()
        );
        return Err(4);
    }
    if (start.has_upper != 0) != ymm {
        // CR4.OSXSAVE or XCR0 changed between _start and now: CPUs disagree.
        println!(
            "vecstress {seed}: ymm state was {} at _start but is {} now",
            if start.has_upper != 0 { "on" } else { "off" },
            if ymm { "on" } else { "off" }
        );
        return Err(4);
    }
    Ok(())
}

/// What is not in its initial state, if anything: x87 control word 0x037F, status
/// 0, every tag empty, no instruction or data pointer, ST0-7, xmm0-15 and the upper
/// ymm halves zero, MXCSR 0x1F80. The pointer check cannot see AMD's leak on CPUs
/// without XSaveErPtr (their FXSAVE drops the pointers too, and TCG never stores
/// them); the kernel's x87 reset covers that by construction.
fn dirty_start(start: &Start) -> Option<&'static str> {
    let fx = &start.fx;
    let u16_at = |at: usize| u16::from_le_bytes([fx[at], fx[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes([fx[at], fx[at + 1], fx[at + 2], fx[at + 3]]);
    let zero = |range: core::ops::Range<usize>| fx[range].iter().all(|&b| b == 0);
    if u16_at(0) != FCW_INIT {
        Some("x87 control word")
    } else if u16_at(2) != 0 || fx[4] != 0 {
        Some("x87 status or tags")
    } else if !zero(6..24) {
        Some("x87 instruction or data pointer")
    } else if u32_at(24) != MXCSR_INIT {
        Some("MXCSR")
    } else if !(0..8).all(|st| zero(32 + 16 * st..32 + 16 * st + 10)) {
        Some("an x87 register")
    } else if !zero(160..416) {
        Some("an xmm register")
    } else if start.upper.iter().flatten().any(|&b| b != 0) {
        Some("the upper half of a ymm register")
    } else {
        None
    }
}

/// splitmix64's finaliser: a different 64-bit value for every input.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Fills in this round's pattern, MXCSR (rounding control and flush-to-zero vary,
/// every exception stays masked), x87 control word (precision and rounding vary),
/// x87 value and system call.
fn prepare(block: &mut Block, seed: u64, round: u64) {
    let base = (seed << 40) ^ (round << 16);
    for (reg, lanes) in block.pattern.iter_mut().enumerate() {
        for (lane, value) in lanes.iter_mut().enumerate() {
            *value = mix(base ^ ((reg as u64) << 8) ^ lane as u64) as u32;
        }
    }
    let k = seed.wrapping_add(round);
    block.mxcsr_in = MXCSR_INIT | (((k % 4) as u32) << 13) | (((k / 4 % 2) as u32) << 15);
    let precision: u16 = [0b00, 0b10, 0b11][(k % 3) as usize];
    block.fcw_in = 0x007F | (precision << 8) | (((k / 3 % 4) as u16) << 10);
    block.x87_in = mix(base ^ 0xFFFF) as i64;
    (block.call, block.arg) = match round % 4 {
        0 => (kabi::call::TIME, 0),
        1 => (kabi::call::YIELD, 0),
        // SLEEP 0 returns at once, so both sleeps are real.
        2 => (kabi::call::SLEEP, 1),
        _ => (kabi::call::SLEEP, 2),
    };
}

/// What did not come back as loaded, if anything.
fn lost(block: &Block, mode: Mode) -> Option<Lost> {
    let lanes = match mode {
        Mode::Xmm => XMM_LANES,
        Mode::Ymm | Mode::Avx2 => YMM_LANES,
    };
    let added = mode != Mode::Ymm;
    for reg in 0..REGS {
        for lane in 0..lanes {
            let mut want = block.pattern[reg][lane];
            if added && reg < REGS - 1 {
                want = want.wrapping_add(block.pattern[REGS - 1][lane]);
            }
            let got = block.out[reg][lane];
            if got != want {
                return Some(Lost::Lane {
                    reg,
                    lane,
                    got,
                    want,
                });
            }
        }
    }
    if block.mxcsr_out != block.mxcsr_in {
        Some(Lost::Mxcsr(block.mxcsr_out, block.mxcsr_in))
    } else if block.fcw_out != block.fcw_in {
        Some(Lost::Fcw(block.fcw_out, block.fcw_in))
    } else if block.fsw_out & FSW_CHECKED != 0 {
        Some(Lost::Fsw(block.fsw_out))
    } else if block.x87_out != block.x87_in {
        Some(Lost::X87(block.x87_out, block.x87_in))
    } else {
        None
    }
}

/// The first thing a round found changed.
enum Lost {
    Lane {
        reg: usize,
        lane: usize,
        got: u32,
        want: u32,
    },
    Mxcsr(u32, u32),
    Fcw(u16, u16),
    Fsw(u16),
    X87(i64, i64),
}

impl core::fmt::Display for Lost {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Lost::Lane {
                reg,
                lane,
                got,
                want,
            } => write!(f, "register {reg} lane {lane} is {got:#x}, not {want:#x}"),
            Lost::Mxcsr(got, want) => write!(f, "MXCSR is {got:#x}, not {want:#x}"),
            Lost::Fcw(got, want) => write!(f, "x87 control word is {got:#x}, not {want:#x}"),
            Lost::Fsw(got) => write!(f, "x87 status word is {got:#x}"),
            Lost::X87(got, want) => write!(f, "x87 value is {got}, not {want}"),
        }
    }
}

/// One round in one asm block: keep the caller's MXCSR and control word, load the
/// pattern into every register (and add register 15 to the others, except in plain
/// ymm mode), load MXCSR, the control word and one x87 value, spin, make the system
/// call, spin again, store everything back, then restore what was kept.
macro_rules! round {
    ($block:expr, $spin:expr, [$($load:literal),*], [$($add:literal),*], [$($store:literal),*]) => {
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
            pattern = const offset_of!(Block, pattern),
            out = const offset_of!(Block, out),
            in("r12") $block,
            in("r13") $spin,
            clobber_abi("C"),
            options(nostack),
        )
    };
}

/// # Safety
/// `block` must point to a live Block, and `mode` must match what the CPU and XCR0
/// allow (checked by `agrees_with_cpu`).
unsafe fn run_round(block: *mut Block, spin: u64, mode: Mode) {
    // SAFETY: the asm reads and writes only `*block` (through r12), keeps the x87
    // stack empty at its end, restores MXCSR and the x87 control word, and the
    // kernel preserves r12 and r13 across SYSCALL. The caller vouches for the mode.
    unsafe {
        match mode {
            Mode::Avx2 => round!(
                block,
                spin,
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
