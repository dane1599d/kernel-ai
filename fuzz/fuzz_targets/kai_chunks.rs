//! libFuzzer's `kai_chunks` target: [`harness::kai_chunks`].

#![no_main]

#[path = "../harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::kai_chunks(data));
