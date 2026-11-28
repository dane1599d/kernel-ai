//! libFuzzer's `ivrs_tables` target (plan P6 G61's fuzz half): the input is
//! one IVRS table, read as it is and resealed (length and checksum mended)
//! through [`ivrs_harness::ivrs_tables`]. The body is
//! acpitables/tests/hostile/ivrs.rs, which acpitables' own hostile tests also
//! run.

#![no_main]

#[path = "../../acpitables/tests/hostile/ivrs.rs"]
mod ivrs_harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    ivrs_harness::ivrs_tables(data);
});
