//! libFuzzer's `desk_server` target: [`harness::desk_server`].

#![no_main]

#[path = "../desk_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::desk_server(data);
});
