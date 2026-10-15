//! Pass/fail exit through QEMU's isa-debug-exit device (iobase 0xf4).
//! QEMU exits with status (code << 1) | 1, so Success gives 33 and Failure gives 35.

use crate::port::outl;

const ISA_DEBUG_EXIT: u16 = 0xF4;

#[derive(Clone, Copy)]
#[repr(u32)]
pub enum ExitCode {
    Success = 0x10,
    Failure = 0x11,
}

/// Ends the QEMU run with the given status. Halts forever if the device is absent.
pub fn exit(code: ExitCode) -> ! {
    // SAFETY: in the test configuration port 0xf4 is QEMU's isa-debug-exit device.
    unsafe { outl(ISA_DEBUG_EXIT, code as u32) };
    crate::halt_forever()
}
