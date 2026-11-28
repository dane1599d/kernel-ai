//! libFuzzer's `gguf_file` target (P6 G32): the input is a GGUF file, read as a header and as a whole file. The body is gguf/tests/harness/gguf_file.rs, which
//! gguf's own hostile tests also run.

#![no_main]

#[path = "../../gguf/tests/harness/gguf_file.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::gguf_file(data);
});
