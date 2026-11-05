//! libFuzzer's `url` target: [`harness::url`].

#![no_main]

#[path = "../harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::url(data));
