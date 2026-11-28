//! libFuzzer's `xhci_descriptors` target (ADR 0007 section 12: the first byte picks a device, configuration or HID report descriptor, a boot keyboard report, a hub descriptor or a hub port status, and the rest is its bytes). The body is xhci/tests/hostile/harness.rs, which
//! xhci's own hostile tests also run.

#![no_main]

#[path = "../../xhci/tests/hostile/harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::xhci_descriptors(data);
});
