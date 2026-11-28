//! libFuzzer's `acpi_tables` target (plan P6 G34, and G61's fuzz half): the
//! input is firmware memory with the RSDP at its start, read through
//! [`harness::acpi_tables`] (the MADT, HPET, MCFG and FADT readers), and an
//! IVRS table through [`ivrs_harness::ivrs_tables`]. Their bodies are
//! acpitables/tests/hostile/, which acpitables' own hostile tests also run.

#![no_main]

#[path = "../../acpitables/tests/hostile/harness.rs"]
mod harness;
#[path = "../../acpitables/tests/hostile/ivrs.rs"]
mod ivrs_harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::acpi_tables(data);
    ivrs_harness::ivrs_tables(data);
});
