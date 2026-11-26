//! libFuzzer's `zip_archive` target: [`harness::zip_archive`].

#![no_main]

#[path = "../zip_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| harness::zip_archive(data));
