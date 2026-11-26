# Spike S06-1: the nesting census

ADR 0006 asks whether the GitHub runners we use can host our SVM hypervisor
under nested KVM. The gate is SVM with NPT in at least 18 of 20 KVM runs, and
always under TCG. Branch `spike/nesting-census` is never merged; its `cpufeat`
bits and CI matrix are the parts the ADR keeps.

## What runs

- `.github/workflows/spike-nesting-census.yml`, on push to this branch or by
  workflow_dispatch:
  - 20 runs on `ubuntu-24.04`. Each prints the host's vendor and model,
    `/dev/kvm`, `kvm_amd`/`kvm_intel` `nested` and the host's virtualisation
    flags. It then boots the `nesting-census` case under KVM (`-cpu max`) and
    under TCG.
  - 2 runs on `macos-latest`, which print the host facts only (no KVM, no boot).
  - A summary job puts every run in one table, in the run's step summary.
- `cpufeat::Nesting` reads CPUID.1:ECX bit 5 (VMX), 0x8000_0001 ECX bit 2 (SVM),
  and leaf 0x8000_000A whole. The kernel logs it at boot, after the CPU line:
  `nesting: vendor=AuthenticAMD vmx=0 svm=1 npt=1 nrips=0 decode_assists=0 avic=0 asids=16 leaf_8000000a=eax:...`.
- The `nesting-census` case (xtask/src/cases/kernel/basics.rs) is a plain
  `test` boot that expects that line. `XTASK_CASE=nesting-census` boots it alone.

## Results

Filled in from the workflow run below.
