//! libFuzzer's `fsproto_reply` target: [`harness::reply`].

#![no_main]

#[path = "../fsproto_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::reply(data);
});
