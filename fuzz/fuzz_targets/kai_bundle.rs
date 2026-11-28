//! libFuzzer's `kai_bundle` target (P6 G31): the input is a boot bundle, as it is and with its digests rewritten. The body is kaibundle/tests/hostile/harness.rs, which
//! kaibundle's own hostile tests also run.

#![no_main]

#[path = "../../kaibundle/tests/hostile/harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::kai_bundle(data);
});
