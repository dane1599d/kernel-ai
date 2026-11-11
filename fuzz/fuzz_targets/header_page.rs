//! libFuzzer's `header_page` target: [`harness::header_page`].

#![no_main]

#[path = "../surfabi_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::header_page(data);
});
