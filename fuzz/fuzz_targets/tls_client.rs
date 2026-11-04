//! libFuzzer's `tls_client` target: [`harness::tls_client`].

#![no_main]

#[path = "../harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::tls_client(data));
