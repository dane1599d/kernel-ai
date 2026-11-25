//! libFuzzer's `desk_queues` target: [`harness::desk_queues`].

#![no_main]

#[path = "../desk_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::desk_queues(data);
});
