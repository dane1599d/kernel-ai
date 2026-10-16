//! Kernel AI: M2, memory management.
//!
//! Boots through Limine, logs to COM1 and the framebuffer console, loads the GDT,
//! TSS and IDT, and resolves fault addresses to source lines from the
//! `kernel.lines` module. The kernel command line selects a self-test:
//! `test` (boot checks), `test=page-fault`, `test=double-fault`, `test=write-protect` or
//! `test=stack-overflow`.

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod backtrace;
mod console;
mod faults;
mod gdt;
mod idt;
mod lines;
mod memory;
mod port;
mod qemu;
mod serial;
#[cfg(feature = "spikes")]
mod spikes;

use core::sync::atomic::{AtomicBool, Ordering};
use limine::request::{
    ExecutableCmdlineRequest, FramebufferRequest, HhdmRequest, MemmapRequest, ModulesRequest,
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
        log!("Kernel AI {} (M2)", env!("CARGO_PKG_VERSION"));
        log!(
            "framebuffer: {}x{} {} bpp, console: {}",
            fb.width,
            fb.height,
            fb.bpp,
            ready
        );
    } else {
        log!(
            "Kernel AI {} (M2), no framebuffer",
            env!("CARGO_PKG_VERSION")
        );
    }

    load_line_table();
    gdt::init();
    idt::init();
    init_memory();
    log!("cpu: GDT, TSS (double-fault IST) and IDT loaded");

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
