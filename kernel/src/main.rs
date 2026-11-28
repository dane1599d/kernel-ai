//! Kernel AI: M6, network and terminal.
//!
//! Boots through Limine, logs to COM1 and the framebuffer console, loads the GDT,
//! TSS and IDT, sets up memory, then the APICs, HPET, a 1 kHz timer and the PS/2
//! keyboard and mouse, starts the scheduler, and brings up every other CPU. Fault
//! addresses resolve to source lines from the `kernel.lines` module. The kernel
//! command line selects a self-test: `test` (boot checks), or `test=` one of
//! `page-fault`, `double-fault`, `write-protect`, `stack-overflow`, `timer`,
//! `keyboard`, `pointer`, `sched`, `watchdog`, `smp`, `tlb`, `storm`, `storm-SECONDS`,
//! `net`, `terminal`, `blk`, `model`, `fpu`, `probe` or `icount-probe`. `llm=IP:PORT` and `model=NAME` point the
//! LLM client at a server; `model_len=` and `model_fnv=` describe the file behind the
//! `kai-model` disk; `pointerlog` logs every pointer move and click, and `screenlog` copies
//! each console repaint to serial as `screen:` lines.

#![no_std]
#![no_main]

extern crate alloc;

mod act_tests;
mod agent_desk_tests;
mod allow;
mod apic;
mod app_build_tests;
mod app_editor_build_tests;
mod app_editor_files_tests;
mod app_editor_tests;
mod app_files_tests;
mod app_models_tests;
mod app_monitor_tests;
mod app_notes_tests;
mod app_route_tests;
mod app_settings_tests;
mod app_terminal_tests;
mod app_window_tests;
mod appbench_tests;
mod appearance_tests;
mod ask_box;
mod ask_tests;
mod async_tests;
mod backtrace;
mod bigmap_tests;
mod blk_tests;
mod blkflush_tests;
mod block;
mod boot_manifest;
mod bootfiles;
mod build_job;
mod build_service;
mod card_tests;
mod card_wake_tests;
mod catalog_tests;
mod cc_build_tests;
mod cc_tests;
mod cc_trial_tests;
mod claim_tests;
mod clock;
mod compute_tests;
mod console;
mod cpu;
mod default_tests;
mod demand_tests;
mod desk_many_tests;
mod desk_perf_tests;
mod desk_profile_tests;
mod desk_sends_tests;
mod desk_tests;
mod desk_tour_tests;
mod desk_wheel_app_tests;
mod devhost;
mod device_tests;
mod devices;
mod dispatch;
mod display;
mod display_tests;
mod edit_mode;
mod edit_session;
mod edit_tests;
mod efi;
mod efi_tests;
mod entropy;
mod entropy_tests;
mod entry;
mod event_box;
mod fault;
mod faults;
mod files_tests;
mod founder_walk_tests;
mod fpu;
mod fpu_tests;
mod gdt;
mod handle;
mod handle_tests;
mod hpet;
mod https_tests;
mod i8042;
mod icount_tests;
mod idt;
mod input;
mod interrupt_tests;
mod ipc;
mod ipc_queued_tests;
mod ipc_rtt_tests;
mod irq;
mod keep_tests;
mod keyboard;
mod keys_tests;
mod latency_tests;
mod lines;
mod live_commands;
mod live_tests;
mod llm;
mod llm_commands;
mod local_tests;
mod logbook;
mod manifest_tests;
mod many_tests;
mod mem_tests;
mod memory;
mod metrics;
mod model;
mod model_fill_tests;
mod model_import_tests;
mod model_tests;
mod models_window_tests;
mod nap_busy_tests;
mod nap_tests;
mod net;
mod net_commands;
mod net_download_tests;
mod net_grant_tests;
mod net_hold_tests;
mod net_pf_tests;
mod net_stream_tests;
mod net_tests;
mod net_throughput_tests;
mod nvme;
mod pane_states_tests;
mod part_tests;
mod percpu;
mod pkg_tests;
mod pkgrepo;
mod platform;
mod pointer;
mod port;
mod power;
mod power_tests;
mod process;
mod profile_floor_tests;
mod profile_tests;
mod qemu;
mod saved_tests;
mod sched_control;
mod sched_tests;
mod script_tests;
mod scriptrun_tests;
mod selftest;
mod serial;
mod shell_tests;
mod shm;
mod shm_tests;
mod show_tests;
mod slots;
mod smp;
mod smp_tests;
#[cfg(feature = "spikes")]
mod spikes;
mod standing_tests;
mod start_tests;
mod stop_tests;
mod suspend_tests;
mod term_tests;
mod terminal;
mod thread_tests;
mod threads;
mod timer;
mod tufbench_tests;
mod tune_ab_tests;
mod tune_local_tests;
mod tune_stall_tests;
mod tune_tests;
mod ui_tests;
mod usage_tests;
mod usb;
mod version;
mod virtio;
mod wait_reads_tests;
mod wake_tests;
mod watchdog;
mod workload;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use limine::request::{
    ExecutableCmdlineRequest, FramebufferRequest, HhdmRequest, MemmapRequest, ModulesRequest,
    MpRequest, RsdpRequest,
};
use limine::{BaseRevision, RequestsEndMarker, RequestsStartMarker};
use qemu::ExitCode;
use selftest::SelfTest;

#[used]
#[unsafe(link_section = ".requests_start_marker")]
static REQUESTS_START: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static CMDLINE: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
pub(crate) static MODULES: ModulesRequest = ModulesRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP: MemmapRequest = MemmapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static HHDM: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static RSDP: RsdpRequest = RsdpRequest::new();

/// Application processors, in xAPIC mode (flag 0: no x2APIC).
#[used]
#[unsafe(link_section = ".requests")]
static MP: MpRequest = MpRequest::new(0);

#[used]
#[unsafe(link_section = ".requests_end_marker")]
static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

/// Set when the kernel command line selects a self-test; outcomes then exit QEMU.
static TEST_MODE: AtomicBool = AtomicBool::new(false);
/// Number of CPUs Limine reported, for the boot check.
static REPORTED_CPUS: AtomicUsize = AtomicUsize::new(1);

/// The kernel command line, or "" without one.
fn cmdline() -> &'static str {
    CMDLINE.response().map_or("", |r| r.cmdline())
}

/// Whether the command line selects a self-test: a test boot, whose test
/// words (`llm.cold_ms=`) count; any other boot ignores them.
fn test_boot() -> bool {
    TEST_MODE.load(Ordering::SeqCst)
}

/// Runs one self-test case and reports its outcome to QEMU.
fn run_case(check: impl FnOnce() -> Result<(), &'static str>) -> ! {
    if let Err(message) = check() {
        fail(message);
    }
    // Debug builds count each lock a `ready()` took that is held elsewhere
    // with interrupts on: one could have frozen two CPUs.
    if let Some((count, lock)) = threads::ready::ready_locks() {
        log!("threads: a ready() took {lock}, {count} such locks in all");
        fail("a ready() took a lock that is held with interrupts on");
    }
    test_exit(ExitCode::Success)
}

/// Kernel entry point, called by Limine.
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    percpu::init(0);
    serial::init();
    let test = CMDLINE
        .response()
        .map_or(SelfTest::None, |r| selftest::from_cmdline(r.cmdline()));
    TEST_MODE.store(test != SelfTest::None, Ordering::SeqCst);

    if !BASE_REVISION.is_supported() {
        fail("Limine base revision 6 is not supported by this bootloader");
    }
    if let Some(fb) = FRAMEBUFFER
        .response()
        .and_then(|r| r.framebuffers().first().copied())
    {
        let ready = console::init(fb);
        log!("{}", version::Banner);
        log!(
            "framebuffer: {}x{} {} bpp, console: {}",
            fb.width,
            fb.height,
            fb.bpp,
            ready
        );
    } else {
        log!("{}, no framebuffer", version::Banner);
    }

    load_line_table();
    gdt::init();
    idt::init();
    init_memory();
    log!("cpu: GDT, TSS (double-fault IST) and IDT loaded");
    move_to_boot_stack()
}

/// Pages in the boot thread's stack. Limine's boot stack is only guaranteed 64 KiB
/// and has no guard page; the debug-build interpreters in the spikes need more.
const BOOT_STACK_PAGES: u64 = 63;

/// Leaves Limine's stack for a large, guarded one, then continues booting there.
fn move_to_boot_stack() -> ! {
    let stack = match memory::stack::GuardedStack::new(BOOT_STACK_PAGES) {
        Ok(stack) => stack,
        Err(message) => fail(message),
    };
    // SAFETY: stack.top is the top of a freshly mapped stack; boot_continue never returns.
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "xor rbp, rbp",
            "call {next}",
            top = in(reg) stack.top,
            next = sym boot_continue,
            options(noreturn)
        )
    }
}

/// Second boot stage, on the guarded boot stack.
extern "C" fn boot_continue() -> ! {
    let test = CMDLINE
        .response()
        .map_or(SelfTest::None, |r| selftest::from_cmdline(r.cmdline()));
    if test == SelfTest::TuneAb {
        console::status::keep_off();
    }
    if test == SelfTest::DisplayClaim {
        // QEMU's GOP needs no shadow; this case runs the shadow path on it.
        display::force_shadow();
    }
    // Not in kmain: configuring allocates, and the heap is mapped only by init_memory.
    if let Some(cmdline) = CMDLINE.response() {
        net::setup::configure(cmdline.cmdline());
        llm::configure(cmdline.cmdline());
        build_service::configure(cmdline.cmdline());
        pkgrepo::configure(cmdline.cmdline());
        boot_manifest::configure(cmdline.cmdline());
        pointer::configure(cmdline.cmdline());
        console::configure(cmdline.cmdline());
    }
    match cpu::init(entry::syscall_entry as *const () as u64) {
        Ok(features) => log!(
            "cpu: ring 3 ready; SMEP {}, SMAP {}, UMIP {}; {} on, XCR0 {:#x}, XSAVE area {} bytes",
            on_off(features.smep),
            on_off(features.smap),
            on_off(features.umip),
            fpu::state_name(),
            fpu::xcr0(),
            fpu::area_size()
        ),
        Err(message) => fail(message),
    }
    log!("{}", fpu::features_line());
    // The nesting census (P6 row 1, ADR 0006): the VMX and SVM bits this guest
    // sees, one grep-able line; nesting-census.yml tabulates it per runner.
    log!(
        "nesting: vendor={} {}",
        core::str::from_utf8(&cpufeat::vendor()).unwrap_or("?"),
        cpufeat::Nesting::detect()
    );
    // Before the other CPUs start: SetVirtualAddressMap runs on a borrowed PML4.
    if let Err(message) = efi::init() {
        log!("{message}; no runtime services");
    }
    let platform = init_devices();
    if let Err(message) = threads::init() {
        fail(message);
    }
    start_other_cpus();
    virtio::init(platform, cmdline());
    // Before start_user_space sets the page budget, so the model's frames are
    // outside every quota.
    model::load(cmdline());
    start_user_space(test);

    dispatch::run(test);

    #[cfg(feature = "spikes")]
    if let Err(message) = spikes::run() {
        fail(message);
    }
    if test == SelfTest::Boot {
        boot_self_test();
    }

    log!("boot-ok");
    if test != SelfTest::None {
        test_exit(ExitCode::Success);
    }
    log!("Ready on {} CPU(s).", smp::online());
    terminal::run(0);
    halt_forever()
}

/// Brings up every application processor Limine reported.
fn start_other_cpus() {
    let Some(mp) = MP.response() else {
        log!("smp: Limine reported no other CPUs");
        return;
    };
    REPORTED_CPUS.store(mp.cpus().len(), Ordering::Relaxed);
    match smp::start_aps(mp.cpus(), mp.bsp_lapic_id) {
        Ok(online) => log!("smp: {online} CPU(s) online"),
        Err(message) => fail(message),
    }
}

/// ACPI, local APIC, HPET, the 1 kHz timer and the PS/2 ports, then interrupts on.
fn init_devices() -> &'static acpitables::Platform {
    let hhdm = HHDM.response().map_or(0, |r| r.offset);
    let Some(rsdp) = RSDP.response().map(|r| r.address as u64) else {
        fail("Limine did not provide the ACPI RSDP");
    };
    let result = platform::init(rsdp, hhdm).and_then(|platform| {
        // First, so any later failure's reboot finds the register mapped.
        power::init(platform);
        let apic_id = apic::init_local(platform)?;
        smp::set_boot_lapic(apic_id);
        hpet::init(platform)?;
        hpet::select_clock(cmdline());
        if cmdline().split_whitespace().any(|word| word == "stamp=ms") {
            serial::stamp_lines(true);
            log!("serial: each line starts with [the kernel clock h the HPET], in seconds");
        }
        timer::init()?;
        entropy::init();
        // ADR 0007 section 8: no 8042 when the FADT says the machine has none.
        if platform.no_8042 {
            log!("i8042: the FADT says this machine has no 8042; skipped");
        } else {
            i8042::init(platform, apic_id)?;
        }
        log!(
            "devices: local APIC id {apic_id}, {} I/O APIC(s), {} IRQ override(s)",
            platform.io_apics.len(),
            platform.overrides.len()
        );
        Ok(platform)
    });
    match result {
        Ok(platform) => {
            x86_64::instructions::interrupts::enable();
            platform
        }
        Err(message) => fail(message),
    }
}

/// Sets up frames, page tables and the heap. Nothing may allocate before this runs.
fn init_memory() {
    let entries = MEMMAP.response().map(|r| r.entries());
    let hhdm = HHDM.response().map(|r| r.offset);
    let (Some(entries), Some(hhdm)) = (entries, hhdm) else {
        fail("Limine did not provide a memory map and HHDM offset");
    };
    if let Err(message) = memory::init(entries, hhdm) {
        fail(message);
    }
}

fn load_line_table() {
    if let Err(message) = bootfiles::open() {
        fail(message);
    }
    let Some(table) = bootfiles::find("kernel.lines") else {
        log!("lines: kernel.lines not on the boot drive; faults will show raw addresses");
        return;
    };
    match lines::init(table) {
        Ok(()) => {
            if let Some(table) = lines::table() {
                log!(
                    "lines: {} line rows, {} symbols",
                    table.line_count(),
                    table.symbol_count()
                );
            }
        }
        Err(error) => log!("lines: cannot parse kernel.lines: {error:?}"),
    }
}

/// Boot checks: breakpoints resume, addresses resolve to source, and memory works.
fn boot_self_test() {
    let before = faults::breakpoint_count();
    x86_64::instructions::interrupts::int3();
    if faults::breakpoint_count() != before + 1 {
        fail("breakpoint was not caught and resumed");
    }
    let resolved = lines::resolve(faults::self_check_address());
    let file = resolved.and_then(|location| location.file);
    if !file.is_some_and(|f| f.ends_with("kernel/src/faults.rs")) {
        fail("line table did not resolve a known function to kernel/src/faults.rs");
    }
    log!("self-test: breakpoint resumed, source lookup ok");
    if let Err(message) = cpu::same_on_every_cpu(smp::online()) {
        fail(message);
    }
    if let Err(message) = smp_tests::boot_check(REPORTED_CPUS.load(Ordering::Relaxed)) {
        fail(message);
    }
    if let Err(message) = sched_tests::boot_check() {
        fail(message);
    }
    if let Err(message) = timer::boot_check() {
        fail(message);
    }
    if let Err(message) = memory::selftest::run() {
        fail(message);
    }
}

/// Reports the outcome to QEMU in test mode; otherwise halts.
pub fn test_exit(code: ExitCode) -> ! {
    if TEST_MODE.load(Ordering::SeqCst) {
        qemu::exit(code);
    }
    halt_forever()
}

/// Ends the boot after a fault: reset (outside test mode, or with `panic=reset`),
/// or the failure exit a test run reports.
pub(crate) fn fault_exit(reason: &dyn core::fmt::Display) -> ! {
    if fault::resets(TEST_MODE.load(Ordering::SeqCst)) {
        fault::reset(reason);
    }
    qemu::exit(ExitCode::Failure)
}

/// Logs a fatal error and stops: a reset, or the test run's failure exit.
fn fail(message: &str) -> ! {
    smp::begin_fatal_report();
    log!("FATAL on cpu {}: {message}", percpu::index());
    backtrace::print_from(backtrace::current_rbp());
    fault_exit(&message)
}

/// Disables interrupts and halts the CPU forever.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: cli; hlt only stops this CPU.
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    smp::begin_fatal_report();
    log!("PANIC on cpu {}: {info}", percpu::index());
    log!("backtrace:");
    backtrace::print_from(backtrace::current_rbp());
    fault_exit(&info.message())
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Finds the ring-3 programs among the boot modules and, for the boots that use
/// user space, starts supervision with the service plan.
fn start_user_space(test: SelfTest) {
    bootfiles::init();
    process::components::init(bootfiles::all());
    process::programs::init(bootfiles::all());
    let programs = process::programs::all();
    log!("process: {} programs loaded as modules", programs.len());
    let free = memory::with(|m| m.free_frames());
    process::set_page_budget(free / 2);
    let Some(plan) = selftest::plan(test) else {
        return;
    };
    if programs.is_empty() {
        return;
    }
    // The icount boots start their services on a whole second of ticks, after
    // every boot-time disk read, so nothing host-timed comes before them and every
    // boot starts them at the same virtual time (the M10 exit test).
    if test == SelfTest::TuneAb {
        // The waits before the edge were host-timed: the Cores forget them on
        // both sides of it, so no stall carries over (sched::Core::resync_ticks).
        threads::resync_ticks();
        let edge = timer::align_to_second();
        threads::resync_ticks();
        log!("tune-ab: services start at tick {edge}");
    }
    // Ctrl+Alt+F12 switches the screen between the console and `desktop`.
    if let Err(message) = display::claim::init() {
        fail(message);
    }
    if let Err(message) = process::supervision::start(plan) {
        fail(message);
    }
    if let Err(message) = build_service::start_sync() {
        fail(message);
    }
    if let Err(message) = boot_manifest::start() {
        fail(message);
    }
}
