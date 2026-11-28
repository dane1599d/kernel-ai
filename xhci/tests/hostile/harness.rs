//! The `xhci_descriptors` fuzz target's body (ADR 0007 section 12, P8 row 1),
//! which `xhci/tests/hostile.rs` also runs over hostile descriptors: a device
//! plugged in, or a hub it sits behind, hands the kernel these bytes, parsed
//! in ring 0. `fuzz/fuzz_targets/xhci_descriptors.rs` includes this file by
//! `#[path]` and calls [`xhci_descriptors`] with libFuzzer's input. It must
//! return for any input, and only a bug in `xhci` may panic.
//!
//! The input's first byte picks the parser (so each gets its own bytes) and
//! the rest is what it reads: a device descriptor, a configuration, a HID
//! report descriptor, a boot keyboard report, a hub descriptor or a hub
//! port's status. What a parser takes keeps its range: at most `MAX_HID`
//! interfaces, a field read only inside its report, a hub of at least one port
//! with a power-good time and think time in range, and a speed the xHCI names.

#![allow(dead_code)]

use xhci::Error;
use xhci::descriptor::{
    Configuration, Device, HubDescriptor, MAX_HID, PortStatus, configuration_length,
};
use xhci::hid::{Field, Keyboard, pointer_layout};
use xhci::regs::speed;

/// Reports of every length a device could send a pointer: none, short, the
/// usual, and long, each with every byte set.
pub const REPORTS: [&[u8]; 6] = [
    &[],
    &[0xFF],
    &[0x02, 0xFF, 0xFF],
    &[0xFF; 6],
    &[0x01; 8],
    &[0xFF; 64],
];

/// The parsers' selectors, the first byte of an input modulo [`KINDS`].
pub const DEVICE: u8 = 0;
pub const CONFIGURATION: u8 = 1;
pub const REPORT_DESCRIPTOR: u8 = 2;
pub const KEYBOARD: u8 = 3;
pub const HUB: u8 = 4;
pub const PORT_STATUS: u8 = 5;
pub const KINDS: u8 = 6;

pub fn xhci_descriptors(data: &[u8]) {
    let Some((&kind, bytes)) = data.split_first() else {
        return;
    };
    match kind % KINDS {
        DEVICE => device(bytes),
        CONFIGURATION => configuration(bytes),
        REPORT_DESCRIPTOR => report_descriptor(bytes),
        KEYBOARD => keyboard(bytes),
        HUB => hub(bytes),
        _ => port_status(bytes),
    }
}

/// A field never reads past the data it is given: `read` gives None instead.
pub fn field(field: Option<Field>, data: &[u8]) {
    let Some(field) = field else {
        return;
    };
    if field.read(data).is_some() {
        let end = u64::from(field.offset) + u64::from(field.size);
        assert!(
            end <= 8 * data.len() as u64,
            "{field:?} read past {} bytes",
            data.len()
        );
    }
}

pub fn report_descriptor(descriptor: &[u8]) {
    let Some(layout) = pointer_layout(descriptor) else {
        return;
    };
    for report in REPORTS {
        let _ = layout.read(report);
        let data = match layout.report_id {
            Some(id) => report.strip_prefix(&[id]).unwrap_or(&[]),
            None => report,
        };
        for f in [layout.x, layout.y, layout.wheel] {
            field(f, data);
        }
    }
}

pub fn configuration(bytes: &[u8]) {
    let _ = configuration_length(bytes);
    if let Ok(config) = Configuration::parse(bytes) {
        assert!(config.interfaces().count() <= MAX_HID);
    }
}

pub fn device(bytes: &[u8]) {
    let _ = Device::max_packet0(bytes);
    let _ = Device::parse(bytes);
}

pub fn keyboard(report: &[u8]) {
    let mut keyboard = Keyboard::new();
    let mut keys = 0;
    keyboard.report(&[0; 8], |_, _| keys += 1);
    keyboard.report(report, |_, _| keys += 1);
    keyboard.report(&[0; 8], |_, _| keys += 1);
    // 8 modifiers and 6 keys pressed, then released, at the most.
    assert!(keys <= 2 * (8 + 6), "{keys} key changes from one report");
}

/// A hub descriptor, which needs its first 7 bytes and a hub of one port or
/// more; one taken has a power-good time of 2 ms per byte at most, and a think
/// time of 0 to 3.
pub fn hub(bytes: &[u8]) {
    match HubDescriptor::parse(bytes) {
        Ok(hub) => {
            assert!(bytes.len() >= 7, "a short hub descriptor was taken");
            assert!(hub.ports >= 1);
            assert!(hub.power_good_ms <= 2 * 255);
            assert!(hub.think_time <= 3);
        }
        Err(error) => assert_eq!(error, Error::BadDescriptor),
    }
}

/// A hub port's status, four bytes; the speed it names is one the xHCI has.
pub fn port_status(bytes: &[u8]) {
    match PortStatus::parse(bytes) {
        Ok(port) => {
            assert!(bytes.len() >= 4, "a short port status was taken");
            let _ = (port.connected(), port.enabled(), port.reset_done());
            assert!([speed::LOW, speed::HIGH, speed::FULL].contains(&port.speed()));
        }
        Err(error) => assert_eq!(error, Error::BadDescriptor),
    }
}
