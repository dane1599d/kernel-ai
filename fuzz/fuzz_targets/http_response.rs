//! libFuzzer's `http_response` target: [`harness::http_response`].

#![no_main]

#[path = "../harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::http_response(data));
