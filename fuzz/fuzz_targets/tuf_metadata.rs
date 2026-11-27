//! libFuzzer's `tuf_metadata` target: [`harness::tuf_metadata`].

#![no_main]

#[path = "../pkg_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::tuf_metadata(data);
});
