//! Kernel AI: M0 boot and harness.
//!
//! Boots through Limine, logs to COM1, fills the framebuffer, optionally runs the
//! Rhai and wasmi spikes, and in test mode reports pass or fail to QEMU.

#![no_std]
#![no_main]

#[cfg(feature = "spikes")]
extern crate alloc;

mod framebuffer;
mod port;
mod qemu;
mod serial;
#[cfg(feature = "spikes")]
mod spikes;

use core::sync::atomic::{AtomicBool, Ordering};
use limine::request::{ExecutableCmdlineRequest, FramebufferRequest};
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
#[unsafe(link_section = ".requests_end_marker")]
static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

/// Set when the kernel command line contains `test`; failures then exit QEMU.
static TEST_MODE: AtomicBool = AtomicBool::new(false);

/// Kernel entry point, called by Limine.
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    serial::init();
    log!("Kernel AI {} (M0) booting", env!("CARGO_PKG_VERSION"));

    let test_mode = CMDLINE
        .response()
        .is_some_and(|r| r.cmdline().split_whitespace().any(|arg| arg == "test"));
    TEST_MODE.store(test_mode, Ordering::Relaxed);

    if !BASE_REVISION.is_supported() {
        fail("Limine base revision 6 is not supported by this bootloader");
    }
    log!(
        "limine: base revision {:?}",
        BASE_REVISION.actual_revision()
    );

    match FRAMEBUFFER
        .response()
        .and_then(|r| r.framebuffers().first().copied())
    {
        Some(fb) => {
            let filled = framebuffer::fill(fb);
            log!(
                "framebuffer: {}x{} {} bpp, filled: {}",
                fb.width,
                fb.height,
                fb.bpp,
                filled
            );
        }
        None => log!("framebuffer: none"),
    }

    #[cfg(feature = "spikes")]
    if let Err(message) = spikes::run() {
        fail(message);
    }

    log!("boot-ok");
    if test_mode {
        qemu::exit(ExitCode::Success);
    }
    halt_forever()
}

/// Logs a fatal error; in test mode also reports failure to QEMU.
fn fail(message: &str) -> ! {
    log!("FATAL: {}", message);
    if TEST_MODE.load(Ordering::Relaxed) {
        qemu::exit(ExitCode::Failure);
    }
    halt_forever()
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
    log!("PANIC: {}", info);
    if TEST_MODE.load(Ordering::Relaxed) {
        qemu::exit(ExitCode::Failure);
    }
    halt_forever()
}
