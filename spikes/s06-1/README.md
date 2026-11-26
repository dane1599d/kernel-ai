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

Run [45734622772](https://github.com/dane1599d/kernel-ai/actions/runs/45734622772)
(2026-11-25, commit af7c856d, QEMU 11.0.5): all 22 jobs passed.

### What each runner offers

| Runner | Host CPU | Runs | `/dev/kvm` | `nested` | Guest under KVM (`-cpu max`) | Guest under TCG |
|---|---|---|---|---|---|---|
| ubuntu-24.04 | Intel Xeon 6975P-C | 12 of 20 | yes | kvm_intel `Y` | VMX; no SVM | AMD: SVM, NPT; no NRIPS |
| ubuntu-24.04 | AMD EPYC 9V74 80-core | 6 of 20 | yes | kvm_amd `1` | SVM rev 1, 16 ASIDs, NPT, NRIPS; no decode assists or AVIC | the same as above |
| ubuntu-24.04 | AMD EPYC 7763 64-core | 2 of 20 | yes | kvm_amd `1` | the same as the 9V74 | the same as above |
| macos-latest | Apple M1 (Virtual), arm64 | 2 of 2 | no | none | not booted (no x86 KVM) | not booted |

Raw leaf 0x8000_000A EDX:
- under KVM on AMD: `0x10009479`;
- under TCG: `0x10010001`, on every run.

On the AMD hosts, `/proc/cpuinfo` shows `decodeassists`, but KVM does not pass
it to the guest.

### Against the gate

- **SVM with NPT in at least 18 of 20 KVM runs: failed.** Only 8 of 20 had it,
  and these were exactly the runs on AMD hosts. The other 12 were Intel and
  offered nested VMX instead (kvm_intel `nested=Y`, so the guest sees VMX).
- **SVM with NPT always under TCG: passed,** 20 of 20. NRIPS was off in all 20,
  as ADR 0006 says for QEMU's TCG.

The ADR's fail column says that with mostly Intel runners, VMX moves into P6
(+1-2 PM). Every runner has KVM and offers nesting on its own vendor, so an
SVM-only hypervisor could be tested under KVM on about 40% of runs and under
TCG on all.

### Not measured

- **VMX capabilities.** EPT and unrestricted guest live in the VMX capability
  MSRs, which only ring 0 can read. The census logs CPUID only, and the host
  shows `ept`. Reading the MSRs needs a kernel `rdmsr` step.
- **VirtualBox.** The "once in VirtualBox with nested VT-x/AMD-V on" run is the
  founder's and is not done here.
- **Run-to-run mix.** The 12/8 Intel/AMD split is one run's sample of the pool
  and will vary.
