//! libFuzzer's `fetch_wire` target: [`harness::fetch_wire`].

#![no_main]

#[path = "../harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::fetch_wire(data));
