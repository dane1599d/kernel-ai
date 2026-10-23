//! L2 self-tests, with processes the kernel starts itself (no supervisor).
//!
//! - `test=fpu`: four `vecstress` processes keep their own patterns in every vector
//!   register, MXCSR and the x87 unit through preemption, system calls and
//!   migration, while pinned hogs move around the CPUs so the balancer keeps moving
//!   them. Four more start while the first four's state is live on every CPU, and
//!   each checks it started clean. Where the CPU offers AVX-512 F and BW (KVM on
//!   a runner with them; never TCG, which does not emulate AVX-512) the patterns
//!   fill all 32 zmm registers and k0-7 too; elsewhere the case says zmm was not
//!   checked. The case fails if the CPU reports AVX2 and ymm was not exercised with
//!   it, if it offers AVX-512 and zmm state is off, if a CPU never loaded a
//!   process's vector state, or if no state ever moved between CPUs.
//! - `test=probe`: the isolation probe's ops 0-2 (x87, SSE, AVX) complete, and
//!   every other op is stopped by the exception `kabi::probe` lists.

use crate::percpu::MAX_CPUS;
use crate::process::{self, SpawnRequest, supervision};
use crate::{cpu, fpu, log, smp, threads};
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use kabi::probe::{self, Outcome};
use kabi::{ExitReason, Limits};

/// Processes per wave; the second wave starts while the first is running.
const WAVE: u64 = 4;
const SECOND_WAVE_AFTER_MS: u64 = 150;
const ROUNDS: u64 = 200;
/// TSC cycles each spin in a round lasts, about half a millisecond.
const SPIN_CYCLES: u64 = 2_000_000;
const EXIT_TIMEOUT_MS: u64 = 120_000;
/// Pinned hogs on one CPU at a time, and how long before they move to the next.
const LOAD_HOGS: usize = 2;
const LOAD_PHASE_MS: u64 = 40;
const LIMITS: Limits = Limits {
    max_pages: 1024,
    max_burst_ms: 10_000,
    caps: 0,
    stack_pages: 0,
};

static LOAD_PHASE: AtomicUsize = AtomicUsize::new(0);
static LOAD_STOP: AtomicBool = AtomicBool::new(false);

/// Whether ymm state should be on: the CPU has AVX and XSAVE can hold its state.
/// The cases judge the kernel against this, not against what it turned on.
fn avx_expected(cpu: &cpufeat::Features) -> bool {
    cpu.avx && cpu.xcr0_supported & cpufeat::XCR0_AVX != 0
}

/// Whether zmm and opmask state should be on, with vecstress's zmm mode: the CPU
/// has AVX-512 F and BW and XSAVE can hold all three of its components.
fn avx512_expected(cpu: &cpufeat::Features) -> bool {
    let all = cpufeat::XCR0_AVX512;
    avx_expected(cpu) && cpu.avx512f && cpu.avx512bw && cpu.xcr0_supported & all == all
}

fn yes(on: bool) -> &'static str {
    if on { "yes" } else { "no" }
}

/// The `fpu` case.
pub fn fpu_check() -> Result<(), &'static str> {
    let cpu = cpufeat::Features::detect();
    let ymm = cpu.avx && cpu.ymm_enabled();
    let avx2 = cpu.avx2 && ymm;
    let zmm = cpu.avx512();
    log!(
        "fpu: XCR0 {:#x}, XSAVE area {} bytes; ymm {}, avx2 {}, zmm and opmask {}",
        fpu::xcr0(),
        fpu::area_size(),
        yes(ymm),
        yes(avx2),
        yes(zmm)
    );
    if Some(cpu.xcr0) != cpufeat::ring3_xcr0(cpu.xcr0_supported) || cpu.xcr0 != fpu::xcr0() {
        return Err("fpu: XCR0 is not the one cpufeat chooses for this CPU");
    }
    if cpu.avx2 && !ymm {
        return Err("fpu: the CPU reports AVX2 but ymm state is not on");
    }
    if avx_expected(&cpu) != ymm {
        return Err("fpu: the CPU has AVX but ymm state is not on");
    }
    if avx512_expected(&cpu) != zmm {
        return Err("fpu: the CPU offers AVX-512 but zmm and opmask state is not on");
    }
    if !zmm {
        log!("fpu: this CPU offers no AVX-512 F and BW; zmm and opmask state not checked");
    }
    let cpus = smp::online();
    let (restores_before, moved_before) = fpu::restores();
    let migrations_before = threads::migrations();
    if cpus > 1 {
        threads::spawn("fpu-load", rotate_load, cpus)?;
    }
    let mut pids = [0; 2 * WAVE as usize];
    for seed in 1..=WAVE {
        pids[seed as usize - 1] = start_vecstress(seed, ymm, avx2, zmm)?;
    }
    threads::sleep_ms(SECOND_WAVE_AFTER_MS);
    for seed in WAVE + 1..=2 * WAVE {
        pids[seed as usize - 1] = start_vecstress(seed, ymm, avx2, zmm)?;
    }
    let outcome = pids.iter().try_for_each(|&pid| match wait_exit(pid)? {
        (ExitReason::Exited, 0) => Ok(()),
        (reason, detail) => {
            log!("fpu: pid {pid} ended: {} ({detail})", reason.name());
            Err("fpu: a vecstress process lost its state or failed")
        }
    });
    LOAD_STOP.store(true, Ordering::Release);
    outcome?;
    let (restores_after, moved_after) = fpu::restores();
    let restores: [u64; MAX_CPUS] =
        core::array::from_fn(|i| restores_after[i] - restores_before[i]);
    let moved = moved_after - moved_before;
    let mode = match (ymm, avx2, zmm) {
        (true, true, true) => "zmm and opmask with AVX-512",
        (true, true, false) => "ymm with AVX2",
        (true, false, _) => "ymm",
        _ => "xmm",
    };
    log!(
        "fpu: {} processes kept their {mode} state; restores per CPU {:?}, {moved} from another CPU, {} migrations",
        2 * WAVE,
        &restores[..cpus],
        threads::migrations() - migrations_before
    );
    if restores[..cpus].contains(&0) {
        return Err("fpu: a CPU never loaded a process's vector state");
    }
    if cpus > 1 && moved == 0 {
        return Err("fpu: no process's vector state moved between CPUs");
    }
    Ok(())
}

fn start_vecstress(seed: u64, ymm: bool, avx2: bool, zmm: bool) -> Result<u64, &'static str> {
    let args = format!(
        "seed={seed} rounds={ROUNDS} spin={SPIN_CYCLES} ymm={} avx2={} zmm={}",
        u8::from(ymm),
        u8::from(avx2),
        u8::from(zmm)
    );
    start(&format!("vec{seed}"), "vecstress", &args)
}

/// Moves extra load around the CPUs, `LOAD_HOGS` pinned hogs on one CPU at a time,
/// so the balancer keeps pulling runnable vecstress threads off the busy CPU.
fn rotate_load(cpus: usize) {
    let mut phase = 0;
    while !LOAD_STOP.load(Ordering::Acquire) {
        for _ in 0..LOAD_HOGS {
            // A full thread table only means less load this phase.
            let _ = threads::spawn_pinned(phase % cpus, "fpu-hog", load_hog, phase);
        }
        threads::sleep_ms(LOAD_PHASE_MS);
        phase += 1;
        LOAD_PHASE.store(phase, Ordering::Release);
    }
}

/// Spins until the load moves to the next CPU.
fn load_hog(phase: usize) {
    while LOAD_PHASE.load(Ordering::Acquire) == phase {
        core::hint::spin_loop();
    }
}

/// The `probe` case.
pub fn probe_check() -> Result<(), &'static str> {
    let umip = cpu::features().umip;
    let avx = avx_expected(&cpufeat::Features::detect());
    for (op, (name, _)) in probe::OPS.iter().enumerate() {
        let pid = start("probe", "probe", &format!("restart={op}"))?;
        let outcome = match wait_exit(pid)? {
            (ExitReason::Exited, code) => Outcome::Exited(code),
            (ExitReason::Crashed, vector) => Outcome::Stopped(vector),
            _ => Outcome::Other,
        };
        log!("probe: op {op} ({name}): {outcome:?}");
        if !probe::allowed(op, outcome, umip, avx) {
            return Err("probe: an op ended the wrong way");
        }
    }
    log!(
        "probe: ops 0-2 completed and the other {} ops were stopped (UMIP {}, AVX {})",
        probe::OPS.len() - 3,
        yes(umip),
        yes(avx)
    );
    Ok(())
}

/// Starts a process the kernel itself watches.
fn start(name: &str, program: &str, args: &str) -> Result<u64, &'static str> {
    process::spawn(&SpawnRequest {
        name,
        program,
        args: args.as_bytes(),
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|error| {
        log!("self-test: cannot start {name}: {error:?}");
        "self-test: cannot start a test process"
    })
}

/// Waits for a process the kernel started to end, and says how it did.
pub(crate) fn wait_exit(pid: u64) -> Result<(ExitReason, u64), &'static str> {
    for _ in 0..EXIT_TIMEOUT_MS / 10 {
        if let Some(exit) = supervision::kernel_child_exit(pid) {
            return Ok(exit);
        }
        threads::sleep_ms(10);
    }
    supervision::kernel_child_exit(pid).ok_or("self-test: a test process did not end")
}
