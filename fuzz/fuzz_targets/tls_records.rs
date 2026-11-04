//! libFuzzer's `tls_records` target: [`harness::tls_records`].

#![no_main]

#[path = "../harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::tls_records(data));
