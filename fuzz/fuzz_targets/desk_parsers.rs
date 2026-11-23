//! libFuzzer's `desk_parsers` target: [`harness::desk_parsers`].

#![no_main]

#[path = "../desk_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::desk_parsers(data);
});
