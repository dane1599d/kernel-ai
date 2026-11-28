//! libFuzzer's `acpi_tables` target (plan P6 G34): the input is firmware
//! memory with the RSDP at its start, read through [`harness::acpi_tables`]
//! (the MADT, HPET, MCFG and FADT readers). IVRS has its own target,
//! `ivrs_tables`. The body is acpitables/tests/hostile/harness.rs, which
//! acpitables' own hostile tests also run.

#![no_main]

#[path = "../../acpitables/tests/hostile/harness.rs"]
mod harness;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    harness::acpi_tables(data);
});
