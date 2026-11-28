//! libFuzzer's `dev_cfg` target (ADR 0007 section 5 and 12, P8 row 1): the
//! input is a write to a function's own configuration space and that space as
//! the function reads, and an allowed write must change no BAR, no MSI or
//! MSI-X byte, no capability header and no command bit but Memory Space and
//! Bus Master. The body is pci/tests/hostile/harness.rs, which pci's own
//! hostile test also runs.

#![no_main]

#[path = "../../pci/tests/hostile/harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::dev_cfg(data);
});
