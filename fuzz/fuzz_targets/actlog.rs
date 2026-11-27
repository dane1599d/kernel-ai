//! libFuzzer's `actlog` target: [`harness::actlog`].

#![no_main]

#[path = "../actlog_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::actlog(data);
});
