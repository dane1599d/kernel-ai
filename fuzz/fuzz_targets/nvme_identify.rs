//! libFuzzer's `nvme_identify` target (ADR 0007 section 12: the input is read as an Identify Controller, an Identify Namespace and an active-namespace list page). The body is nvme/tests/hostile/harness.rs, which
//! nvme's own hostile tests also run.

#![no_main]

#[path = "../../nvme/tests/hostile/harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::nvme_identify(data);
});
