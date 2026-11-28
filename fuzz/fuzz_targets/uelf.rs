//! libFuzzer's `uelf` target (P6 G31): the input is an ELF executable read as the kernel loader reads it. The body is uelf/tests/hostile/harness.rs, which
//! uelf's own hostile tests also run.

#![no_main]

#[path = "../../uelf/tests/hostile/harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::uelf(data);
});
