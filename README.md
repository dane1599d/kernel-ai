# Kernel AI

A from-scratch x86_64 operating system in Rust, inspired by TempleOS, with a local LLM built into how the system is used and tuned. Status: milestone M6 (network and terminal: virtio-net, TCP/IP, an LLM client and a command prompt). `cargo xtask run` boots to a command prompt; type `help`.

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
| `llmproto/` | Host-tested HTTP/1.1 and OpenAI chat protocol for the LLM client. |
| `spikes/` | M0 proofs that Rhai and wasmi run on bare metal. |

## Requirements

- Rust via rustup. The pinned nightly in `rust-toolchain.toml` installs automatically.
- QEMU (`qemu-system-x86_64`) and OVMF UEFI firmware. On Windows, the official QEMU installer includes OVMF. Set `QEMU`, `OVMF_CODE` and `OVMF_VARS` if they are not found automatically.
- `curl` and `tar` to fetch Limine on the first run.

## Commands

```bash
cargo xtask build            # build the kernel
cargo xtask run              # boot to the terminal in a QEMU window
cargo xtask run --llm 192.168.1.50:8080 --model qwen3-8b  # with the Mac mini LLM
cargo xtask test             # boots each self-test case headless (14 cases on 4 CPUs, with a mock LLM server)
cargo xtask test --accel kvm # use KVM (Linux or WSL2)
cargo xtask soak             # the 10-minute spawn/exit/sleep storm
cargo test -p xtask -p linetable -p mm -p acpitables -p sched -p llmproto --features linetable/alloc  # host unit tests
```

## Licence

MPL-2.0 (see `docs/decisions.md`).

## Talking to the LLM

The kernel sends `ask` commands to any OpenAI-compatible server, such as llama.cpp's `llama-server` on the Mac mini. Start the server bound to the LAN, then boot with its address:

```bash
cargo xtask run --llm 192.168.1.50:8080 --model qwen3-8b
```

QEMU's user-mode network lets the VM reach LAN addresses directly. Use the Mac's IP address, since the kernel has no DNS.
