//! libFuzzer's `surfabi_stream` target: [`harness::surfabi_stream`].

#![no_main]

#[path = "../surfabi_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::surfabi_stream(data);
});
