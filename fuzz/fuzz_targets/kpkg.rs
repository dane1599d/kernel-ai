//! libFuzzer's `kpkg` target: [`harness::kpkg`].

#![no_main]

#[path = "../pkg_harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::kpkg(data);
});
