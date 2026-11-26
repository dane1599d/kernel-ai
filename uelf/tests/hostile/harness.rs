//! The `uelf` fuzz target's body (plan P6 G31), which `uelf/tests/hostile.rs`
//! also runs over hostile executables, the way `fuzz/harness.rs` serves its
//! targets: a later `fuzz/fuzz_targets/uelf.rs` includes this file by `#[path]`
//! and calls [`uelf`] with libFuzzer's input. It must return for any input, and
//! only a bug in `uelf` may panic.
//!
//! The promise it holds is the one the kernel's loader relies on: a parsed
//! image never maps a page outside `allowed`, never maps a page writable and
//! executable, never maps two segments over each other, copies only file bytes
//! that exist, and enters inside its code.

#![allow(dead_code)]

use core::ops::Range;
use uelf::{Image, MAX_SEGMENTS, parse};

/// The range the kernel's loader passes (`kernel/src/process/loader.rs`):
/// `MIN_USER_ADDRESS..HEAP_BASE` (`kernel/src/memory/user.rs`, `kabi::layout`).
pub const ALLOWED: Range<u64> = 0x1_0000..0x10_0000_0000;
const PAGE: u64 = 4096;

/// Parses `data` with [`ALLOWED`]; an image that comes out keeps every promise.
pub fn uelf(data: &[u8]) {
    if let Ok(image) = parse(data, ALLOWED) {
        check_image(&image, data);
    }
}

/// The promises the loader relies on, for any image `parse` returns.
pub fn check_image(image: &Image, data: &[u8]) {
    let segments = image.segments();
    assert!(!segments.is_empty() && segments.len() <= MAX_SEGMENTS);
    let mut pages = 0;
    for s in segments {
        assert!(s.file.start <= s.file.end && s.file.end <= data.len());
        assert!((s.file.end - s.file.start) as u64 <= s.memsz && s.memsz > 0);
        // Every byte the segment maps, not only its first and last page.
        let end = s.vaddr.checked_add(s.memsz).expect("a segment's end fits");
        assert!(ALLOWED.start <= s.vaddr && end <= ALLOWED.end);
        let p = s.pages();
        assert!(ALLOWED.start <= p.start && p.end <= ALLOWED.end, "{s:?}");
        assert!(p.start % PAGE == 0 && p.end % PAGE == 0 && p.start < p.end);
        assert!(p.start <= s.vaddr && end <= p.end);
        assert!(!(s.write && s.execute));
        pages += (p.end - p.start) / PAGE;
    }
    for (i, a) in segments.iter().enumerate() {
        for b in &segments[i + 1..] {
            let (a, b) = (a.pages(), b.pages());
            assert!(a.end <= b.start || b.end <= a.start);
        }
    }
    assert!(
        segments
            .iter()
            .any(|s| s.execute && s.vaddr <= image.entry && image.entry - s.vaddr < s.memsz)
    );
    assert_eq!(image.page_count(), pages);
}
