//! Kernel AI: M4, threads and preemptive scheduling.
//!
//! Boots through Limine, logs to COM1 and the framebuffer console, loads the GDT,
//! TSS and IDT, sets up memory, then the APICs, HPET, a 1 kHz timer and the PS/2
//! keyboard. Fault addresses resolve to source lines from the `kernel.lines` module.
//! The kernel command line selects a self-test: `test` (boot checks), or
//! `test=` one of `page-fault`, `double-fault`, `write-protect`, `stack-overflow`,
//! `timer`, `keyboard`, `sched`, `watchdog`.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod apic;
mod backtrace;
mod console;
mod faults;
mod gdt;
mod hpet;
mod idt;
mod keyboard;
mod lines;
mod memory;
mod platform;
mod port;
mod qemu;
mod sched_tests;
mod serial;
#[cfg(feature = "spikes")]
mod spikes;
mod threads;
mod timer;

use core::sync::atomic::{AtomicBool, Ordering};
use limine::request::{
    ExecutableCmdlineRequest, FramebufferRequest, HhdmRequest, MemmapRequest, ModulesRequest,
    RsdpRequest,
};
use limine::{BaseRevision, RequestsEndMarker, RequestsStartMarker};
use qemu::ExitCode;

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
static MODULES: ModulesRequest = ModulesRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP: MemmapRequest = MemmapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static HHDM: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static RSDP: RsdpRequest = RsdpRequest::new();

#[used]
#[unsafe(link_section = ".requests_end_marker")]
static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

/// Set when the kernel command line selects a self-test; outcomes then exit QEMU.
static TEST_MODE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelfTest {
    None,
    Boot,
    PageFault,
    DoubleFault,
    WriteProtect,
    StackOverflow,
    Timer,
    Keyboard,
    Sched,
    Watchdog,
}

fn self_test_from(cmdline: &str) -> SelfTest {
    cmdline
        .split_whitespace()
        .find_map(|arg| match arg {
            "test" => Some(SelfTest::Boot),
            "test=page-fault" => Some(SelfTest::PageFault),
            "test=double-fault" => Some(SelfTest::DoubleFault),
            "test=write-protect" => Some(SelfTest::WriteProtect),
            "test=stack-overflow" => Some(SelfTest::StackOverflow),
            "test=timer" => Some(SelfTest::Timer),
            "test=keyboard" => Some(SelfTest::Keyboard),
            "test=sched" => Some(SelfTest::Sched),
            "test=watchdog" => Some(SelfTest::Watchdog),
            _ => None,
        })
        .unwrap_or(SelfTest::None)
}

/// Kernel entry point, called by Limine.
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    serial::init();
    let test = CMDLINE
        .response()
        .map_or(SelfTest::None, |r| self_test_from(r.cmdline()));
    TEST_MODE.store(test != SelfTest::None, Ordering::SeqCst);

    if !BASE_REVISION.is_supported() {
        fail("Limine base revision 6 is not supported by this bootloader");
    }
    if let Some(fb) = FRAMEBUFFER
        .response()
        .and_then(|r| r.framebuffers().first().copied())
    {
        let ready = console::init(fb);
        log!("Kernel AI {} (M4)", env!("CARGO_PKG_VERSION"));
        log!(
            "framebuffer: {}x{} {} bpp, console: {}",
            fb.width,
            fb.height,
            fb.bpp,
            ready
        );
    } else {
        log!(
            "Kernel AI {} (M4), no framebuffer",
            env!("CARGO_PKG_VERSION")
        );
    }

    load_line_table();
    gdt::init();
    idt::init();
    init_memory();
    log!("cpu: GDT, TSS (double-fault IST) and IDT loaded");
    let platform = init_devices();
    if let Err(message) = threads::init() {
        fail(message);
    }

    match test {
        SelfTest::PageFault => {
            faults::trigger_page_fault();
            fail("page fault did not trap");
        }
        SelfTest::DoubleFault => faults::trigger_double_fault(),
        SelfTest::WriteProtect => {
            let result = memory::selftest::trigger_write_protect();
            fail(result.err().unwrap_or("write-protect test returned"));
        }
        SelfTest::StackOverflow => {
            let result = memory::selftest::trigger_stack_overflow();
            fail(result.err().unwrap_or("stack-overflow test returned"));
        }
        SelfTest::Timer => {
            if let Err(message) = timer::sixty_second_check() {
                fail(message);
            }
            test_exit(ExitCode::Success);
        }
        SelfTest::Keyboard => {
            if let Err(message) = keyboard::typing_check() {
                fail(message);
            }
            test_exit(ExitCode::Success);
        }
        SelfTest::Sched => {
            if let Err(message) = sched_tests::latency_check() {
                fail(message);
            }
            test_exit(ExitCode::Success);
        }
        SelfTest::Watchdog => {
            if let Err(message) = sched_tests::watchdog_check() {
                fail(message);
            }
            test_exit(ExitCode::Success);
        }
        SelfTest::Boot | SelfTest::None => {}
    }

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
    log!(
        "Ready. {} CPU(s) reported by firmware. Typing echoes here.",
        platform.cpus.len()
    );
    keyboard::echo_forever()
}

/// ACPI, local APIC, HPET, the 1 kHz timer and the keyboard, then interrupts on.
fn init_devices() -> &'static acpitables::Platform {
    let hhdm = HHDM.response().map_or(0, |r| r.offset);
    let Some(rsdp) = RSDP.response().map(|r| r.address as u64) else {
        fail("Limine did not provide the ACPI RSDP");
    };
    let result = platform::init(rsdp, hhdm).and_then(|platform| {
        let apic_id = apic::init_local(platform)?;
        let hpet = platform
            .hpet
            .ok_or("no HPET; a PIT fallback is not implemented yet")?;
        hpet::init(hpet)?;
        timer::init()?;
        keyboard::init(platform, apic_id)?;
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
    let module = MODULES.response().and_then(|r| {
        r.modules()
            .iter()
            .find(|file| file.path().ends_with("kernel.lines"))
    });
    let Some(module) = module else {
        log!("lines: kernel.lines module not loaded; faults will show raw addresses");
        return;
    };
    match lines::init(module.data()) {
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

/// Logs a fatal error and stops.
fn fail(message: &str) -> ! {
    log!("FATAL: {message}");
    backtrace::print_from(backtrace::current_rbp());
    test_exit(ExitCode::Failure)
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
    log!("PANIC: {info}");
    log!("backtrace:");
    backtrace::print_from(backtrace::current_rbp());
    test_exit(ExitCode::Failure)
}
