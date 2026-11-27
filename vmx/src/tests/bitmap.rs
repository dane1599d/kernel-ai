//! The MSR bitmap's four quarters (vol. 3C 25.6.9) and the I/O bitmaps' two
//! pages (25.6.4), with the wrap rule of 26.1.3.

use crate::bitmap::{BITMAP_SIZE, IoBitmaps, MsrBitmap, msr_bit};
use core::mem::{align_of, size_of};

#[test]
fn msr_bits_are_in_their_quarter() {
    assert_eq!(msr_bit(0x0000_0000), Some((0x000, 0)));
    assert_eq!(msr_bit(0x0000_0010), Some((0x002, 0)));
    assert_eq!(msr_bit(0x0000_1FFF), Some((0x3FF, 7)));
    assert_eq!(msr_bit(0xC000_0080), Some((0x410, 0)));
    assert_eq!(msr_bit(0xC000_1FFF), Some((0x7FF, 7)));
    // Past the ranges: always exits.
    for msr in [0x2000, 0xC000_2000, 0xC001_0000, 0x4000_0000, u32::MAX] {
        assert_eq!(msr_bit(msr), None);
    }
    assert_eq!(
        (size_of::<MsrBitmap>(), align_of::<MsrBitmap>()),
        (4096, 4096)
    );
}

#[test]
fn allowing_an_msr_opens_only_its_direction() {
    let mut map = MsrBitmap::intercept_all();
    assert_eq!(map.intercepts(0xC000_0080), (true, true));
    map.allow(0xC000_0080, true, false).unwrap();
    assert_eq!(map.intercepts(0xC000_0080), (false, true));
    // The write bit is at 0xC00 + 0x10: still set.
    assert_eq!(map.0[0xC10] & 1, 1);
    map.allow(0x10, false, true).unwrap();
    assert_eq!(map.intercepts(0x10), (true, false));
    assert_eq!(map.0[0x802] & 1, 0);
    // Neighbours are untouched, and an MSR outside the map is refused.
    assert_eq!(map.intercepts(0x11), (true, true));
    assert_eq!(map.allow(0xC001_0000, true, true), Err(0xC001_0000));
    assert_eq!(map.intercepts(0xC001_0000), (true, true));
}

#[test]
fn io_ports_split_between_the_two_pages() {
    let mut io = IoBitmaps::intercept_all();
    io.allow(0x7FFE, 4).unwrap();
    assert!(!io.intercepts(0x7FFE, 4));
    assert_eq!(io.a[0xFFF] & 0xC0, 0);
    assert_eq!(io.b[0] & 0x03, 0);
    // One byte past the opened range exits.
    assert!(io.intercepts(0x8001, 2));
    assert!(io.intercepts(0x7FFD, 1));
    assert_eq!(size_of::<IoBitmaps>(), 2 * BITMAP_SIZE);
}

#[test]
fn an_access_that_wraps_past_0xffff_always_exits() {
    let mut io = IoBitmaps::intercept_all();
    io.allow(0xFFFC, 4).unwrap();
    assert!(!io.intercepts(0xFFFC, 4));
    assert!(io.intercepts(0xFFFE, 4));
    io.allow(0, 4).unwrap();
    assert!(io.intercepts(0xFFFF, 2));
    assert_eq!(io.allow(0xFFFF, 2), Err((0xFFFF, 2)));
    assert_eq!(io.allow(0, 0), Ok(()));
}
