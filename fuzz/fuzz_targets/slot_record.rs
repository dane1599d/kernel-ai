//! libFuzzer's `slot_record` target (P6 G33): the input is read as `KaiBootState`, as a `kai-slot 1` file and as a `pending` file. The body is bootslot/tests/hostile/harness.rs, which
//! bootslot's own hostile tests also run.

#![no_main]

#[path = "../../bootslot/tests/hostile/harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::slot_record(data);
    harness::slot_file(data);
    harness::pending(data);
});
