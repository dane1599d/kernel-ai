//! libFuzzer's `agent_manifest` target: [`harness::manifest`].

#![no_main]

#[path = "../agent_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::manifest(data);
});
