//! libFuzzer's `kai_theme` target: [`harness::kai_theme`].

#![no_main]

#[path = "../look_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::kai_theme(data);
});
