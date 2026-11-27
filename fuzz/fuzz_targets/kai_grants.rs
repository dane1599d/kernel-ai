//! libFuzzer's `kai_grants` target: [`harness::grants`].

#![no_main]

#[path = "../grants_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::grants(data);
});
