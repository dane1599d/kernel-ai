//! libFuzzer's `fsproto_request` target: [`harness::request`].

#![no_main]

#[path = "../fsproto_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::request(data);
});
