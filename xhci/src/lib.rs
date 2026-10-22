//! A small xHCI (USB 3) host controller driver for USB keyboards, mice and
//! tablets. Everything that does not touch hardware is plain logic that the host
//! unit-tests: the capability, operational, runtime and doorbell registers, the
//! TRBs and the command, event and transfer rings, slot and endpoint contexts,
//! USB descriptors and requests, and HID reports. [`Controller`] puts them
//! together; it reaches DMA memory, the registers and the clock through the
//! [`Hal`] trait, so the same driver can run in the kernel or in a user-space
//! driver process, polled from a timer or woken by an interrupt.
//!
//! The driver brings up the controller, resets each connected root port, gives
//! each device an address, reads its descriptors, configures its HID interfaces
//! and keeps one interrupt-IN transfer queued on each. Hubs, hot-plug,
//! isochronous and bulk transfers are left out: a keyboard and a pointer on root
//! ports are what VirtualBox and most PCs offer.
//!
//! Written from the eXtensible Host Controller Interface specification 1.2, the
//! USB 2.0 specification (chapter 9) and Device Class Definition for HID 1.11
//! with its usage tables; no code was taken from another driver.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod context;
pub mod descriptor;
mod driver;
pub mod hal;
pub mod hid;
pub mod regs;
pub mod ring;
pub mod trb;

pub use driver::{Attached, Controller, DeviceKind, Input, PortReport};
pub use hal::{Dma, Hal};

/// The page size the driver uses for every structure it hands the controller.
pub const PAGE: usize = 4096;

/// Why the driver gave up on a controller or a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The HAL had no DMA memory left, or none the controller can address.
    NoMemory,
    /// The controller did not change state, or answer a command, in time.
    Timeout,
    /// The controller reported an error in USBSTS.
    Halted,
    /// The controller does not take 4 KiB pages.
    PageSize,
    /// A command or transfer completed with this completion code.
    Completion(u8),
    /// A descriptor was too short or inconsistent.
    BadDescriptor,
    /// More devices or endpoints than the driver keeps track of.
    TooMany,
}

impl Error {
    /// A short text for log lines.
    pub fn text(self) -> &'static str {
        match self {
            Error::NoMemory => "out of device memory",
            Error::Timeout => "the controller did not answer in time",
            Error::Halted => "the controller halted with an error",
            Error::PageSize => "the controller does not take 4 KiB pages",
            Error::Completion(code) => trb::completion_text(code),
            Error::BadDescriptor => "a device sent a bad descriptor",
            Error::TooMany => "more devices than the driver keeps",
        }
    }
}
