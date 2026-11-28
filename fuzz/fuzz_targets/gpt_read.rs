//! libFuzzer's `gpt_read` target (P6 G33): the input is a disk image whose GPT is read. The body is diskmap/tests/hostile/harness.rs, which
//! diskmap's own hostile tests also run.

#![no_main]

#[path = "../../diskmap/tests/hostile/harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::gpt_read(data);
});
