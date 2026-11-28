//! The `nvme_identify` fuzz target's body (ADR 0007 section 12, P8 row 1),
//! which `nvme/tests/hostile.rs` also runs over hostile pages:
//! `fuzz/fuzz_targets/nvme_identify.rs` includes this file by `#[path]` and
//! calls [`nvme_identify`] with libFuzzer's input. It must return for any
//! input, and only a bug in `nvme` may panic.
//!
//! The controller hands the kernel 4 KiB Identify pages, and a broken or
//! malicious one may put anything in them. The input is read as each of the
//! three, and what a parser takes keeps its range: strings no longer than
//! their fields, a transfer cap never above the one passed, a block size a
//! power of two from 512 to 64 KiB, and no more namespaces than a page holds.

#![allow(dead_code)]

use nvme::identify::{IDENTIFY_BYTES, active_namespaces};
use nvme::{Controller, Namespace};

/// The largest transfer cap the kernel passes.
pub const CAP: u64 = 1 << 20;

pub fn nvme_identify(data: &[u8]) {
    controller(data);
    namespace(data);
    list(data);
}

/// An Identify Controller page.
pub fn controller(page: &[u8]) {
    if let Ok(controller) = Controller::parse(page) {
        assert!(page.len() >= IDENTIFY_BYTES, "a short page was taken");
        assert!(controller.serial().len() <= 20);
        assert!(controller.model().len() <= 40);
        assert!(controller.atomic_blocks >= 1);
        for page_shift in [12, 16, 27] {
            assert!(controller.max_transfer(page_shift, CAP) <= CAP);
        }
    }
}

/// An Identify Namespace page.
pub fn namespace(page: &[u8]) {
    if let Ok(namespace) = Namespace::parse(page) {
        assert!(page.len() >= IDENTIFY_BYTES, "a short page was taken");
        assert!(namespace.block_bytes.is_power_of_two());
        assert!((512..=65536).contains(&namespace.block_bytes));
    }
}

/// An active namespace ID list page.
pub fn list(page: &[u8]) {
    assert!(active_namespaces(page).count() <= IDENTIFY_BYTES / 4);
}
