//! libFuzzer's `desk_agent` target: [`harness::desk_agent`].

#![no_main]

#[path = "../desk_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::desk_agent(data);
});
