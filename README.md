# Kernel AI

A from-scratch x86_64 operating system in Rust, inspired by TempleOS, with a local LLM built into how the system is used and tuned. Status: milestone M5 (multiprocessing: per-CPU schedulers, load balancing, TLB shootdown). `cargo xtask run` boots to a screen that echoes typing.

- Plan: [docs/roadmap.md](docs/roadmap.md)
- Decisions: [docs/decisions.md](docs/decisions.md)
- Research: [docs/research/](docs/research/)

## Layout

| Path | What it is |
|---|---|
| `kernel/` | The kernel. Boots through Limine and logs to the serial port. |
| `xtask/` | Host tool that builds the kernel and its line table, fetches Limine, and runs QEMU. |
| `linetable/` | Address-to-source table format shared by xtask and the kernel. |
| `mm/` | Host-tested memory logic: frame bitmap and the CLOCK reclaim policy. |
| `acpitables/` | Host-tested ACPI reader for the MADT and HPET tables. |
| `sched/` | Host-tested scheduler: mechanism, policies, watchdog and a simulator. |
| `spikes/` | M0 proofs that Rhai and wasmi run on bare metal. |

## Requirements

- Rust via rustup. The pinned nightly in `rust-toolchain.toml` installs automatically.
- QEMU (`qemu-system-x86_64`) and OVMF UEFI firmware. On Windows, the official QEMU installer includes OVMF. Set `QEMU`, `OVMF_CODE` and `OVMF_VARS` if they are not found automatically.
- `curl` and `tar` to fetch Limine on the first run.

## Commands

```bash
cargo xtask build            # build the kernel
cargo xtask run              # boot it in a QEMU window
cargo xtask test             # boots each self-test case headless (12 cases on 4 CPUs)
cargo xtask test --accel kvm # use KVM (Linux or WSL2)
cargo xtask soak             # the 10-minute spawn/exit/sleep storm
cargo test -p xtask -p linetable -p mm -p acpitables -p sched --features linetable/alloc  # host unit tests
```

## Licence

MPL-2.0 (see `docs/decisions.md`).
