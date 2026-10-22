use super::*;
use std::vec;

fn changes(keyboard: &mut Keyboard, report: [u8; 8]) -> ([(u8, bool); 8], usize) {
    let mut seen = [(0, false); 8];
    let mut count = 0;
    keyboard.report(&report, |usage, down| {
        seen[count] = (usage, down);
        count += 1;
    });
    (seen, count)
}

#[test]
fn boot_reports_become_presses_and_releases() {
    let mut keyboard = Keyboard::new();
    // Shift down with "k".
    let (seen, count) = changes(&mut keyboard, [0x02, 0, 0x0E, 0, 0, 0, 0, 0]);
    assert_eq!(&seen[..count], &[(0xE1, true), (0x0E, true)]);
    // "k" up, "a" down, Shift still held.
    let (seen, count) = changes(&mut keyboard, [0x02, 0, 0x04, 0, 0, 0, 0, 0]);
    assert_eq!(&seen[..count], &[(0x0E, false), (0x04, true)]);
    // A repeat of the same report changes nothing.
    let (_, count) = changes(&mut keyboard, [0x02, 0, 0x04, 0, 0, 0, 0, 0]);
    assert_eq!(count, 0);
    // Everything up: the modifier goes first.
    let (seen, count) = changes(&mut keyboard, [0; 8]);
    assert_eq!(&seen[..count], &[(0xE1, false), (0x04, false)]);
}

#[test]
fn rollover_reports_are_ignored() {
    let mut keyboard = Keyboard::new();
    changes(&mut keyboard, [0, 0, 0x04, 0, 0, 0, 0, 0]);
    let (_, count) = changes(&mut keyboard, [0, 0, 1, 1, 1, 1, 1, 1]);
    assert_eq!(count, 0);
    let (seen, count) = changes(&mut keyboard, [0; 8]);
    assert_eq!(&seen[..count], &[(0x04, false)]);
}

#[test]
fn usages_map_to_set_one() {
    let code = |usage, down| set1(usage, down).map(|s| s.bytes().to_vec());
    assert_eq!(code(0x04, true), Some(vec![0x1E])); // A
    assert_eq!(code(0x04, false), Some(vec![0x9E]));
    assert_eq!(code(0x1D, true), Some(vec![0x2C])); // Z
    assert_eq!(code(0x1E, true), Some(vec![0x02])); // 1
    assert_eq!(code(0x27, true), Some(vec![0x0B])); // 0
    assert_eq!(code(0x28, true), Some(vec![0x1C])); // Enter
    assert_eq!(code(0x2C, true), Some(vec![0x39])); // Space
    assert_eq!(code(0x38, true), Some(vec![0x35])); // /
    assert_eq!(code(0x45, true), Some(vec![0x58])); // F12
    assert_eq!(code(0x4A, true), Some(vec![0xE0, 0x47])); // Home
    assert_eq!(code(0x52, false), Some(vec![0xE0, 0xC8])); // Up released
    assert_eq!(code(0x58, true), Some(vec![0xE0, 0x1C])); // keypad Enter
    assert_eq!(code(0x63, true), Some(vec![0x53])); // keypad .
    assert_eq!(code(0x65, true), Some(vec![0xE0, 0x5D])); // Application
    assert_eq!(code(0xE0, true), Some(vec![0x1D])); // left Control
    assert_eq!(code(0xE4, true), Some(vec![0xE0, 0x1D])); // right Control
    assert_eq!(code(0xE5, false), Some(vec![0xB6])); // right Shift
    assert_eq!(code(0x46, true), None); // Print Screen
    assert_eq!(code(0x00, true), None);
    assert_eq!(code(0x66, true), None);
}

#[test]
fn boot_mouse_reports_are_signed_movements() {
    assert_eq!(
        boot_mouse(&[0x01, 0xFE, 5]),
        Some(Pointer {
            buttons: 1,
            x: -2,
            y: 5,
            wheel: 0,
            range: None
        })
    );
    assert_eq!(boot_mouse(&[0x04, 1, 1, 0xFF]).unwrap().wheel, -1);
    assert_eq!(boot_mouse(&[0, 0]), None);
}

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

#[test]
fn a_tablet_descriptor_gives_absolute_x_and_y() {
    let layout = pointer_layout(QEMU_TABLET).expect("a pointer");
    assert_eq!(layout.report_id, None);
    assert!(layout.absolute());
    assert_eq!(layout.buttons[..3], [Some(0), Some(1), Some(2)]);
    assert_eq!(layout.buttons[3], None);
    let x = layout.x.unwrap();
    assert_eq!((x.offset, x.size, x.min, x.max), (8, 16, 0, 0x7FFF));
    assert_eq!(layout.y.unwrap().offset, 24);
    let wheel = layout.wheel.unwrap();
    assert!(wheel.relative);
    assert_eq!((wheel.offset, wheel.min), (40, -127));
    // Left button, X 0x4000 (half way), Y 0x2000, wheel -1.
    let report = [0x01, 0x00, 0x40, 0x00, 0x20, 0xFF];
    assert_eq!(
        layout.read(&report),
        Some(Pointer {
            buttons: 1,
            x: 0x4000,
            y: 0x2000,
            wheel: -1,
            range: Some((0x7FFF, 0x7FFF)),
        })
    );
    assert_eq!(layout.read(&report[..3]), None);
}

/// A composite device: a keyboard on report 1 and a mouse with 12-bit relative
/// X and Y on report 2, the X and Y after a Push of the globals.
#[rustfmt::skip]
const COMBO: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01,             // keyboard, ID 1
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0xC0,
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02,             // mouse, ID 2
    0x09, 0x01, 0xA1, 0x00,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x05, 0x81, 0x02, 0x75, 0x03, 0x95, 0x01, 0x81, 0x03,
    0xA4,                                                       // Push
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x16, 0x01, 0xF8, 0x26, 0xFF, 0x07,
    0x75, 0x0C, 0x95, 0x02, 0x81, 0x06,
    0xB4,                                                       // Pop
    0xC0, 0xC0,
];

#[test]
fn report_ids_and_relative_twelve_bit_fields() {
    let layout = pointer_layout(COMBO).expect("a pointer");
    assert_eq!(layout.report_id, Some(2));
    assert!(!layout.absolute());
    assert_eq!(layout.buttons[4], Some(4));
    let x = layout.x.unwrap();
    assert_eq!((x.offset, x.size, x.min, x.max), (8, 12, -2047, 2047));
    assert_eq!(layout.y.unwrap().offset, 20);
    // Right button; X = -3 (0xFFD), Y = +5 (0x005), packed as 12-bit fields.
    let report = [0x02, 0x02, 0xFD, 0x5F, 0x00];
    let pointer = layout.read(&report).unwrap();
    assert_eq!((pointer.buttons, pointer.x, pointer.y), (2, -3, 5));
    assert_eq!(pointer.range, None);
    // A keyboard report is not the pointer's.
    assert_eq!(layout.read(&[0x01, 0x02]), None);
}

#[test]
fn descriptors_without_a_pointer_give_none() {
    // The keyboard collection alone.
    assert_eq!(pointer_layout(&COMBO[..25]), None);
    // Cut off inside an item.
    assert_eq!(pointer_layout(&QEMU_TABLET[..41]), None);
    assert_eq!(pointer_layout(&[]), None);
}

#[test]
fn fields_past_the_report_read_as_none() {
    let field = Field {
        offset: 4,
        size: 8,
        min: 0,
        max: 255,
        relative: false,
    };
    assert_eq!(field.read(&[0xF0, 0x0A]), Some(0xAF));
    assert_eq!(field.read(&[0xF0]), None);
}
