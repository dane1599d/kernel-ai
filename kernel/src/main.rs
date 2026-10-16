//! Kernel AI: M1, CPU exceptions and console.
//!
//! Boots through Limine, logs to COM1 and the framebuffer console, loads the GDT,
//! TSS and IDT, and resolves fault addresses to source lines from the
//! `kernel.lines` module. The kernel command line selects a self-test:
//! `test` (boot checks), `test=page-fault` or `test=double-fault`.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

#[cfg(feature = "spikes")]
extern crate alloc;

mod backtrace;
mod console;
mod faults;
mod gdt;
mod idt;
mod lines;
mod port;
mod qemu;
mod serial;
#[cfg(feature = "spikes")]
mod spikes;

use core::sync::atomic::{AtomicBool, Ordering};
use limine::request::{ExecutableCmdlineRequest, FramebufferRequest, ModulesRequest};
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
}

fn self_test_from(cmdline: &str) -> SelfTest {
    cmdline
        .split_whitespace()
        .find_map(|arg| match arg {
            "test" => Some(SelfTest::Boot),
            "test=page-fault" => Some(SelfTest::PageFault),
            "test=double-fault" => Some(SelfTest::DoubleFault),
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
        log!("Kernel AI {} (M1)", env!("CARGO_PKG_VERSION"));
        log!(
            "framebuffer: {}x{} {} bpp, console: {}",
            fb.width,
            fb.height,
            fb.bpp,
            ready
        );
    } else {
        log!(
            "Kernel AI {} (M1), no framebuffer",
            env!("CARGO_PKG_VERSION")
        );
    }

    load_line_table();
    gdt::init();
    idt::init();
    log!("cpu: GDT, TSS (double-fault IST) and IDT loaded");

    match test {
        SelfTest::PageFault => {
            faults::trigger_page_fault();
            fail("page fault did not trap");
        }
        SelfTest::DoubleFault => faults::trigger_double_fault(),
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
    halt_forever()
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

/// M1 boot checks: a breakpoint is caught and resumed, and addresses resolve to source.
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
