//! Test program for the kernel's `fpu` case. It first checks that it started with
//! clean vector registers, then for many rounds loads a pattern of its own into all
//! sixteen vector registers, MXCSR and the x87 unit, spins, makes a system call,
//! spins again and checks every bit survived preemption, the call and any migration
//! to another CPU. Everything between loading and checking is one asm block, so no
//! compiler-generated code ever touches the registers under test.
//!
//! Arguments: `seed=N` picks the pattern (each process gets its own), `rounds=N`,
//! `spin=N` TSC cycles per spin, and `ymm=1`, `avx2=1` and `zmm=1` say what the
//! kernel found, which the program checks against CPUID itself. With ymm it uses all
//! 256 bits of every register; with avx2 it also adds the registers with VPADDD, and
//! without ymm it adds the xmm registers with PADDD. With zmm (AVX-512 F and BW, and
//! XCR0's opmask and zmm state) it uses all 512 bits of all 32 registers, adding
//! zmm31 to the others, and the eight opmask registers.
//!
//! Exit codes: 0 every round kept its state, 1 bad arguments, 2 dirty at start,
//! 3 state lost, 4 the CPU disagrees with the kernel's arguments.

#![no_std]
#![no_main]

#[path = "vecstress/round.rs"]
mod round;

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
const ZMM_LANES: usize = 16;
const REGS: usize = 16;
const ZMM_REGS: usize = 32;

user::abi_note!();

/// What `_start` saved before any Rust code ran: FXSAVE64's 512 bytes (x87, MXCSR
/// and xmm0-15), the upper halves of ymm0-15, and whether it could read them; with
/// AVX-512 also zmm16-31 whole, bits 256-511 of zmm0-15 and k0-7.
#[repr(C, align(64))]
struct Start {
    fx: [u8; 512],
    upper: [[u8; 16]; REGS],
    hi16: [[u8; 64]; 16],
    zupper: [[u8; 32]; REGS],
    k: [u64; 8],
    has_upper: u8,
    has_zmm: u8,
}

#[repr(transparent)]
struct StartCell(UnsafeCell<Start>);

// SAFETY: this process has one thread; `_start` writes the cell before `main` runs,
// and `main` only reads it.
unsafe impl Sync for StartCell {}

static START: StartCell = StartCell(UnsafeCell::new(Start {
    fx: [0; 512],
    upper: [[0; 16]; REGS],
    hi16: [[0; 64]; 16],
    zupper: [[0; 32]; REGS],
    k: [0; 8],
    has_upper: 0,
    has_zmm: 0,
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
        "mov esi, eax",
        "and eax, 6",
        "cmp eax, 6",
        "jne 2f",
        ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
        "vextractf128 [rdi + {upper} + 16*\\reg], ymm\\reg, 1",
        ".endr",
        "mov byte ptr [rdi + {has_upper}], 1",
        // zmm and k0-7 exist if XCR0 has opmask and both zmm components, and CPUID
        // leaf 7 has AVX-512 F (EBX bit 16) and BW (bit 30), which KMOVQ needs.
        "and esi, 0xE0",
        "cmp esi, 0xE0",
        "jne 2f",
        "mov eax, 7",
        "xor ecx, ecx",
        "cpuid",
        "and ebx, 0x40010000",
        "cmp ebx, 0x40010000",
        "jne 2f",
        ".irp reg, 16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31",
        "vmovdqu32 [rdi + {hi16} + 64*(\\reg-16)], zmm\\reg",
        ".endr",
        ".irp reg, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
        "vextracti64x4 [rdi + {zupper} + 32*\\reg], zmm\\reg, 1",
        ".endr",
        ".irp reg, 0,1,2,3,4,5,6,7",
        "kmovq [rdi + {k} + 8*\\reg], k\\reg",
        ".endr",
        "mov byte ptr [rdi + {has_zmm}], 1",
        "2:",
        "and rsp, -16",
        "call {run}",
        "ud2",
        start = sym START,
        upper = const offset_of!(Start, upper),
        hi16 = const offset_of!(Start, hi16),
        zupper = const offset_of!(Start, zupper),
        k = const offset_of!(Start, k),
        has_upper = const offset_of!(Start, has_upper),
        has_zmm = const offset_of!(Start, has_zmm),
        run = sym run,
    )
}

extern "C" fn run() -> ! {
    sys::exit(main())
}

/// The zmm registers and then k0-7, as the zmm round loads and stores them.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct ZRegs {
    zmm: [[u32; ZMM_LANES]; ZMM_REGS],
    k: [u64; 8],
}

// The zmm round finds k0-7 at this fixed offset after the zmm registers.
const _: () = assert!(offset_of!(ZRegs, k) == 2048);

impl ZRegs {
    const ZERO: ZRegs = ZRegs {
        zmm: [[0; ZMM_LANES]; ZMM_REGS],
        k: [0; 8],
    };
}

/// One round's pattern, what came back, and the control values, at offsets the asm
/// blocks name. The xmm and ymm modes use `pattern` and `out`, the zmm mode `zin`
/// and `zout`.
#[repr(C, align(64))]
struct Block {
    pattern: [[u32; YMM_LANES]; REGS],
    out: [[u32; YMM_LANES]; REGS],
    zin: ZRegs,
    zout: ZRegs,
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
    Zmm,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Xmm => "xmm",
            Mode::Ymm => "ymm",
            Mode::Avx2 => "ymm with avx2",
            Mode::Zmm => "zmm and opmask",
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
    let mode = match (ymm, avx2, number("zmm").unwrap_or(0)) {
        (0, 0, 0) => Mode::Xmm,
        (1, 0, 0) => Mode::Ymm,
        (1, 1, 0) => Mode::Avx2,
        (1, 1, 1) => Mode::Zmm,
        _ => {
            println!("vecstress {seed}: avx2=1 needs ymm=1, and zmm=1 needs avx2=1");
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
        zin: ZRegs::ZERO,
        zout: ZRegs::ZERO,
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
        unsafe { round::run_round(&mut block, spin, mode) };
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
/// the upper ymm halves exactly when ymm state is on, and zmm and k0-7 exactly when
/// AVX-512 F and BW may run.
fn agrees_with_cpu(seed: u64, mode: Mode, start: &Start) -> Result<(), i32> {
    let cpu = cpufeat::Features::detect();
    let ymm = cpu.avx && cpu.ymm_enabled();
    let zmm = cpu.avx512();
    let found = match (ymm, cpu.avx2 && ymm, zmm) {
        (false, _, _) => Mode::Xmm,
        (true, false, _) => Mode::Ymm,
        (true, true, false) => Mode::Avx2,
        (true, true, true) => Mode::Zmm,
    };
    if (start.has_zmm != 0) != zmm {
        println!(
            "vecstress {seed}: zmm state was {} at _start but is {} now",
            if start.has_zmm != 0 { "on" } else { "off" },
            if zmm { "on" } else { "off" }
        );
        return Err(4);
    }
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
/// 0, every tag empty, no instruction or data pointer, ST0-7, xmm0-15, the upper
/// ymm halves, zmm16-31, the upper zmm halves and k0-7 zero, MXCSR 0x1F80. The
/// pointer check cannot see AMD's leak on CPUs
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
    } else if start.hi16.iter().flatten().any(|&b| b != 0) {
        Some("one of zmm16-31")
    } else if start.zupper.iter().flatten().any(|&b| b != 0) {
        Some("the upper half of a zmm register")
    } else if start.k.iter().any(|&k| k != 0) {
        Some("an opmask register")
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
    for (reg, lanes) in block.zin.zmm.iter_mut().enumerate() {
        for (lane, value) in lanes.iter_mut().enumerate() {
            *value = mix(base ^ (1 << 32) ^ ((reg as u64) << 8) ^ lane as u64) as u32;
        }
    }
    for (index, mask) in block.zin.k.iter_mut().enumerate() {
        *mask = mix(base ^ (2 << 32) ^ index as u64);
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
    let (pattern, out) = (&block.pattern, &block.out);
    let vectors = match mode {
        Mode::Xmm => lanes_lost(pattern, out, XMM_LANES, true),
        Mode::Ymm => lanes_lost(pattern, out, YMM_LANES, false),
        Mode::Avx2 => lanes_lost(pattern, out, YMM_LANES, true),
        Mode::Zmm => {
            let (zin, zout) = (&block.zin, &block.zout);
            lanes_lost(&zin.zmm, &zout.zmm, ZMM_LANES, true).or_else(|| {
                (0..zin.k.len())
                    .find(|&i| zout.k[i] != zin.k[i])
                    .map(|i| Lost::Opmask(i, zout.k[i], zin.k[i]))
            })
        }
    };
    if vectors.is_some() {
        vectors
    } else if block.mxcsr_out != block.mxcsr_in {
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

/// The first lane, of the first `lanes` in each register, that did not come back:
/// each register as loaded, plus the last register's lane when `added`.
fn lanes_lost<const N: usize>(
    pattern: &[[u32; N]],
    out: &[[u32; N]],
    lanes: usize,
    added: bool,
) -> Option<Lost> {
    let last = pattern.len() - 1;
    for (reg, (loaded, stored)) in pattern.iter().zip(out).enumerate() {
        for lane in 0..lanes {
            let mut want = loaded[lane];
            if added && reg < last {
                want = want.wrapping_add(pattern[last][lane]);
            }
            let got = stored[lane];
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
    None
}

/// The first thing a round found changed.
enum Lost {
    Lane {
        reg: usize,
        lane: usize,
        got: u32,
        want: u32,
    },
    Opmask(usize, u64, u64),
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
            Lost::Opmask(k, got, want) => write!(f, "k{k} is {got:#x}, not {want:#x}"),
            Lost::Mxcsr(got, want) => write!(f, "MXCSR is {got:#x}, not {want:#x}"),
            Lost::Fcw(got, want) => write!(f, "x87 control word is {got:#x}, not {want:#x}"),
            Lost::Fsw(got) => write!(f, "x87 status word is {got:#x}"),
            Lost::X87(got, want) => write!(f, "x87 value is {got}, not {want}"),
        }
    }
}
