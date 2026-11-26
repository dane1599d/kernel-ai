//! libFuzzer's `media_decode` target: [`harness::media_decode`].

#![no_main]

#[path = "../media_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::media_decode(data));
