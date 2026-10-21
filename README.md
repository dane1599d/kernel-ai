# Kernel AI

A from-scratch x86_64 operating system in Rust, inspired by TempleOS, with a local LLM built into how the system is used and tuned. Status: milestone M6 (network and terminal: virtio-net, TCP/IP, an LLM client and a command prompt). `cargo xtask run` boots to a command prompt; type `help`. M10a, the local model tuning the scheduler (`tune editor` at the prompt), is built but not yet declared done: its exit test waits for the founder's Qwen3-4B recording (see docs/roadmap.md).

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
| `gguf/` | Zero-copy reader for GGUF model files, which it treats as hostile. |
| `cpufeat/` | CPUID and XCR0: which vector instruction sets may run. |
| `quant/` | Q8_0 quantisation and the matrix-vector product, in bit-identical tiers. |
| `bpe/` | Qwen's tokenizer: byte-level BPE, the qwen2 pretokenizer and ChatML. |
| `infer/` | The Qwen3 forward pass and greedy generation, plus generated test models. |

## Requirements

- Rust via rustup. The pinned nightly in `rust-toolchain.toml` installs automatically.
- QEMU (`qemu-system-x86_64`) and OVMF UEFI firmware. On Windows, the official QEMU installer includes OVMF. Set `QEMU`, `OVMF_CODE` and `OVMF_VARS` if they are not found automatically.
- `curl` and `tar` to fetch Limine on the first run.

## Commands

```bash
cargo xtask build            # build the kernel
cargo xtask run              # boot to the terminal in a QEMU window
cargo xtask run --llm 192.168.1.50:8080 --model qwen3-8b  # with an LLM server
cargo xtask test             # boots each self-test case headless (14 cases on 4 CPUs, with a mock LLM server)
cargo xtask test --accel kvm # use KVM (Linux or WSL2)
cargo xtask soak             # the 10-minute spawn/exit/sleep storm
cargo test -p xtask -p gguf -p cpufeat -p quant -p bpe -p infer -p linetable -p mm -p acpitables -p sched -p llmproto --features linetable/alloc  # host unit tests
```

The local model, on the host, through Kernel AI's own engine (no QEMU needed):

```bash
cargo xtask setup            # check this PC, then download Qwen3-0.6B to D:\kai-models and check its SHA-256
cargo xtask model chat "What does an operating system kernel do?"
cargo xtask model check      # re-hash the file and compare with llama.cpp's ids
cargo xtask bench quant      # measure the engine on this CPU
```

## Run it on your PC

Kernel AI needs QEMU with a hardware accelerator to run at a usable speed, above all with the model inside. One command checks this PC and says what to do:

```bash
cargo xtask doctor
```

It changes nothing. It prints a ✓ or ✗ line for each requirement (CPU, RAM, free space, the model store, QEMU for Windows, the Windows Hypervisor Platform, WSL, KVM, QEMU and OVMF inside WSL, `nestedVirtualization`), the fix under each ✗, and then one recommended path and the next command. On Windows the preferred path is WSL2 with KVM, the QEMU and accelerator CI uses:

```bash
cargo xtask run --accel wsl --local qwen3-4b-2507   # QEMU in WSL; its window through WSLg, the serial log here
cargo xtask test --accel wsl                     # every boot test, as CI runs them
```

xtask still builds on Windows; only QEMU runs in the distro (WSL's default, or the one `KAI_WSL_DISTRO` names). A model is copied once into the distro's own disk, `~/.cache/kai-models`, so boots do not read it through `/mnt`. When WSL cannot get KVM, the doctor recommends QEMU for Windows with WHPX (`--accel whpx`) instead.

## Licence

MPL-2.0 (see `docs/decisions.md`).

## Talking to the LLM

Since 2026-10-19 the model is meant to run locally, inside Kernel AI (the Local LLM milestone in `docs/roadmap.md`). Until that lands, and afterwards as CI's test double, the kernel sends `ask` commands to any OpenAI-compatible server, such as llama.cpp's `llama-server`. Start the server bound to the LAN, then boot with its address:

```bash
cargo xtask run --llm 192.168.1.50:8080 --model qwen3-8b
```

QEMU's user-mode network lets the VM reach LAN addresses directly. Use the server's IP address, since the kernel has no DNS.
