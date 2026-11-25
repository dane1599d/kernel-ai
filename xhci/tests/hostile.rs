//! Hostile USB descriptors and HID reports (ADR 0007 section 12): a device
//! plugged in hands the kernel these bytes, parsed in ring 0. Every prefix,
//! every field at every extreme value, and seeded mutations of QEMU's device
//! and configuration descriptors, its mouse and tablet report descriptors, a
//! composite one, and boot keyboard reports: a value or an error, never a
//! panic, no read outside a report, at most `MAX_HID` interfaces, and an
//! allocation bounded by the input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use xhci::descriptor::{Configuration, Device, MAX_HID, configuration_length};
use xhci::hid::{Field, Keyboard, pointer_layout};

const MUTATIONS: usize = 4_000;
const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 4096;

/// QEMU's usb-kbd device descriptor.
const KEYBOARD_DEVICE: [u8; 18] = [
    18, 1, 0x00, 0x02, 0, 0, 0, 64, 0x27, 0x06, 0x01, 0x00, 0, 0, 1, 4, 11, 1,
];

/// QEMU's usb-kbd configuration: one boot keyboard interface.
const KEYBOARD_CONFIG: [u8; 34] = [
    9, 2, 34, 0, 1, 1, 0, 0xA0, 50, // configuration 1
    9, 4, 0, 0, 1, 3, 1, 1, 0, // interface 0: HID, boot, keyboard
    9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63, 0, // HID 1.11, report descriptor 63 bytes
    7, 5, 0x81, 3, 8, 0, 7, // endpoint 1 IN, interrupt, 8 bytes, 7 ms
];

/// Five HID interfaces, one more than are kept (`MAX_HID`).
fn many_interfaces() -> Vec<u8> {
    let mut bytes = vec![9, 2, 0, 0, 5, 1, 0, 0xA0, 50];
    for number in 0..5u8 {
        bytes.extend_from_slice(&[9, 4, number, 0, 1, 3, 1, 2, 0]);
        bytes.extend_from_slice(&[9, 0x21, 0x11, 0x01, 0, 1, 0x22, 52, 0]);
        bytes.extend_from_slice(&[7, 5, 0x81 + number, 3, 4, 0, 10]);
    }
    let len = bytes.len() as u16;
    bytes[2..4].copy_from_slice(&len.to_le_bytes());
    bytes
}

/// QEMU's usb-mouse report descriptor.
#[rustfmt::skip]
const QEMU_MOUSE: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01,
    0x95, 0x05, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x03,
    0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38,
    0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06,
    0xC0, 0xC0,
];

/// QEMU's usb-tablet report descriptor.
#[rustfmt::skip]
const QEMU_TABLET: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25, 0x01,
    0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05,
    0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00,
    0x26, 0xFF, 0x7F, 0x35, 0x00, 0x46, 0xFF, 0x7F, 0x75, 0x10,
    0x95, 0x02, 0x81, 0x02, 0x05, 0x01, 0x09, 0x38, 0x15, 0x81,
    0x25, 0x7F, 0x35, 0x00, 0x45, 0x00, 0x75, 0x08, 0x95, 0x01,
    0x81, 0x06, 0xC0, 0xC0,
];

/// A keyboard on report 1 and a 12-bit relative mouse on report 2, behind a
/// Push and Pop of the globals.
#[rustfmt::skip]
const COMBO: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01,
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0xC0,
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02,
    0x09, 0x01, 0xA1, 0x00,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x05, 0x81, 0x02, 0x75, 0x03, 0x95, 0x01, 0x81, 0x03,
    0xA4,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x16, 0x01, 0xF8, 0x26, 0xFF, 0x07,
    0x75, 0x0C, 0x95, 0x02, 0x81, 0x06,
    0xB4,
    0xC0, 0xC0,
];

/// Reports of every length a device could send a pointer: none, short, the
/// usual, and long, each with every byte set.
const REPORTS: [&[u8]; 6] = [
    &[],
    &[0xFF],
    &[0x02, 0xFF, 0xFF],
    &[0xFF; 6],
    &[0x01; 8],
    &[0xFF; 64],
];

fn bounded<R>(input: usize, f: impl FnOnce() -> R) -> R {
    let (out, peak) = hostile::peak_alloc(f);
    let bound = input * ALLOC_PER_BYTE + ALLOC_FLOOR;
    assert!(peak <= bound, "{peak} bytes for {input} input bytes");
    out
}

/// A field never reads past the data it is given: `read` gives None instead.
fn check_field(field: Option<Field>, data: &[u8]) {
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

fn check_report_descriptor(descriptor: &[u8]) {
    let layout = bounded(descriptor.len(), || pointer_layout(descriptor));
    let Some(layout) = layout else {
        return;
    };
    for report in REPORTS {
        let _ = layout.read(report);
        let data = match layout.report_id {
            Some(id) => report.strip_prefix(&[id]).unwrap_or(&[]),
            None => report,
        };
        for field in [layout.x, layout.y, layout.wheel] {
            check_field(field, data);
        }
    }
}

fn check_configuration(bytes: &[u8]) {
    let _ = configuration_length(bytes);
    if let Ok(config) = bounded(bytes.len(), || Configuration::parse(bytes)) {
        assert!(config.interfaces().count() <= MAX_HID);
    }
}

fn check_device(bytes: &[u8]) {
    let _ = Device::max_packet0(bytes);
    let _ = bounded(bytes.len(), || Device::parse(bytes));
}

fn check_keyboard(report: &[u8]) {
    let mut keyboard = Keyboard::new();
    let mut keys = 0;
    bounded(report.len(), || {
        keyboard.report(&[0; 8], |_, _| keys += 1);
        keyboard.report(report, |_, _| keys += 1);
        keyboard.report(&[0; 8], |_, _| keys += 1);
    });
    // 8 modifiers and 6 keys pressed, then released, at the most.
    assert!(keys <= 2 * (8 + 6), "{keys} key changes from one report");
}

#[test]
fn the_samples_parse_as_built() {
    assert!(Device::parse(&KEYBOARD_DEVICE).is_ok());
    assert_eq!(
        Configuration::parse(&KEYBOARD_CONFIG).map(|c| c.interfaces().count()),
        Ok(1)
    );
    // One HID interface more than are kept: refused, never cut short.
    assert!(Configuration::parse(&many_interfaces()).is_err());
    for descriptor in [QEMU_MOUSE, QEMU_TABLET, COMBO] {
        assert!(pointer_layout(descriptor).is_some());
    }
}

#[test]
fn every_prefix_is_a_value_or_an_error() {
    hostile::truncations(&KEYBOARD_DEVICE, |_, bytes| check_device(bytes));
    for config in [KEYBOARD_CONFIG.to_vec(), many_interfaces()] {
        hostile::truncations(&config, |_, bytes| check_configuration(bytes));
    }
    for descriptor in [QEMU_MOUSE, QEMU_TABLET, COMBO] {
        hostile::truncations(descriptor, |_, bytes| check_report_descriptor(bytes));
    }
    hostile::truncations(&[0x02, 0, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09], |_, r| {
        check_keyboard(r)
    });
}

#[test]
fn every_field_at_every_extreme_stays_in_bounds() {
    hostile::sweep_fields(&KEYBOARD_DEVICE, 0..18, |_, bytes| check_device(bytes));
    for config in [KEYBOARD_CONFIG.to_vec(), many_interfaces()] {
        hostile::sweep_fields(&config, 0..config.len(), |_, b| check_configuration(b));
    }
    for descriptor in [QEMU_MOUSE, QEMU_TABLET, COMBO] {
        hostile::sweep_fields(descriptor, 0..descriptor.len(), |_, b| {
            check_report_descriptor(b)
        });
    }
}

#[test]
fn seeded_mutations_stay_in_bounds() {
    hostile::fuzz(&KEYBOARD_DEVICE, 0x05B0, MUTATIONS, check_device);
    hostile::fuzz(&KEYBOARD_CONFIG, 0x05B1, MUTATIONS, check_configuration);
    hostile::fuzz(&many_interfaces(), 0x05B2, MUTATIONS, check_configuration);
    for (seed, descriptor) in [(0x1D0, QEMU_MOUSE), (0x1D1, QEMU_TABLET), (0x1D2, COMBO)] {
        hostile::fuzz(descriptor, seed, MUTATIONS, check_report_descriptor);
    }
    hostile::fuzz(
        &[0x02, 0, 0x04, 0x05, 0, 0, 0, 0],
        0x0B07,
        MUTATIONS,
        check_keyboard,
    );
}

#[test]
fn fields_at_any_offset_and_size_read_only_inside_the_report() {
    for offset in [0u32, 7, 8, 63, 64, 511, u32::MAX - 40, u32::MAX] {
        for size in [0u8, 1, 8, 12, 16, 31, 32, 33, 255] {
            let field = Field {
                offset,
                size,
                min: -1,
                max: 1,
                relative: true,
            };
            for report in REPORTS {
                check_field(Some(field), report);
            }
        }
    }
}

/// A mouse collection whose X and Y follow a field of `size` bits, `count`
/// times: Usage Page Desktop, Usage Mouse, Collection Application, Report
/// Size and Count (four bytes each), Input, then X and Y as 8-bit fields.
fn huge_fields(size: u32, count: u32) -> Vec<u8> {
    let mut d = vec![0x05, 0x01, 0x09, 0x02, 0xA1, 0x01];
    d.push(0x77);
    d.extend_from_slice(&size.to_le_bytes());
    d.push(0x97);
    d.extend_from_slice(&count.to_le_bytes());
    d.extend_from_slice(&[0x81, 0x01]);
    d.extend_from_slice(&[0x09, 0x30, 0x09, 0x31, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06]);
    d.push(0xC0);
    d
}

#[test]
fn report_offsets_that_would_pass_u32_are_refused() {
    // Each would carry the offset past u32::MAX bits: one huge field, a
    // count of large ones, and one that ends exactly past it.
    for (size, count) in [
        (u32::MAX, 2),
        (u32::MAX, 1),
        (0x8000_0000, 256),
        (0x0100_0000, 256),
        (u32::MAX - 7, 1),
    ] {
        let descriptor = huge_fields(size, count);
        check_report_descriptor(&descriptor);
        assert_eq!(
            pointer_layout(&descriptor),
            None,
            "size {size:#x} x {count}"
        );
    }
    // Large but within u32: a layout, whose X sits past any real report.
    let layout = pointer_layout(&huge_fields(0x0100_0000, 4)).expect("a layout");
    assert_eq!(layout.x.map(|x| x.offset), Some(0x0400_0000));
    check_report_descriptor(&huge_fields(0x0100_0000, 4));
}
