# Kernel AI

A from-scratch x86_64 operating system in Rust, inspired by TempleOS, with a local LLM built into how the system is used and tuned. Status: milestone M1 (CPU exceptions, on-screen console, faults linked to source lines).

- Plan: [docs/roadmap.md](docs/roadmap.md)
- Decisions: [docs/decisions.md](docs/decisions.md)
- Research: [docs/research/](docs/research/)

## Layout

| Path | What it is |
|---|---|
| `kernel/` | The kernel. Boots through Limine and logs to the serial port. |
| `xtask/` | Host tool that builds the kernel and its line table, fetches Limine, and runs QEMU. |
| `linetable/` | Address-to-source table format shared by xtask and the kernel. |
| `spikes/` | M0 proofs that Rhai and wasmi run on bare metal. |

## Requirements

- Rust via rustup. The pinned nightly in `rust-toolchain.toml` installs automatically.
- QEMU (`qemu-system-x86_64`) and OVMF UEFI firmware. On Windows, the official QEMU installer includes OVMF. Set `QEMU`, `OVMF_CODE` and `OVMF_VARS` if they are not found automatically.
- `curl` and `tar` to fetch Limine on the first run.

## Commands

```bash
cargo xtask build            # build the kernel
cargo xtask run              # boot it in a QEMU window
cargo xtask test             # boots each self-test case headless: boot, page fault, double fault
cargo xtask test --accel kvm # use KVM (Linux or WSL2)
cargo test -p xtask -p linetable --features linetable/alloc  # host unit tests
```

## Licence

MPL-2.0 (see `docs/decisions.md`).
