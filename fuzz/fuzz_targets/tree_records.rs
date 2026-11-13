//! libFuzzer's `tree_records` target: [`harness::tree_records`].

#![no_main]

#[path = "../surfabi_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::tree_records(data);
});
