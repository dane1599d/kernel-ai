needed-by: Agents, built 2026-11-04 (lane E; the manager asked for the host half of ADR 0013's spike S13-1 `kv-preempt`; the model lane reviews the engine side)
decision: A preempted slot is kept in RAM as a `Snapshot` beside `save_slot` and `restore_slot`, and a spill file is the `kai-kv 1` file as it is
---
`infer::segments` gets `Snapshot`, `Engine::snapshot_slot` and `Engine::restore_snapshot`: the keys and values are copied as they are (no f16 conversion through a buffer, no hash: RAM never leaves memory), into buffers a pool of snapshots keeps, and a restore checks what a file's checks do except that hash (key, shape, capacity, and a start the tokens share with the prompt), leaving the slot empty on any refusal.

The spill file needs no new format: `kai-kv 1` has the version (its magic), the checksums (FNV-1a 64 of the token ids and of the keys and values) and the owner (its key; `spill_pack_id(agent, request)` makes a `pack_id` of one agent's request, so another's file is refused), and a stopped or flipped file is refused with the slot left empty.

Measured on the host, a release build, Qwen3-4B's shape (36 layers of 1,024 keys and 1,024 values, 144 KiB a position), medians: 512, 1,024 and 2,048 positions take 8.7, 11.3 and 22.1 ms to snapshot and restore into a pooled snapshot (the bar is 50 ms at 1,024), and 480, 1,208 and 3,810 ms for a spill round trip with the write made durable (the bar is 1.5 s at 1,024; the write is the disk's, the restore is 70, 141 and 280 ms). `cargo test -p infer --release preempt -- --ignored --nocapture` prints the table.

Callers that keep several agents' slots call `use_slot` for the agent's own before `snapshot_slot`, which copies the slot in use. A kept snapshot of a full 2,048-position slot is about 288 MiB and counts against `pool_mib` from its first use, since its buffers are kept.

Not in this change: the guest run under KVM with the real model (draft #682, after the v0.1 tag), `llm`'s use of any of it, and the `kai-cache` partition's regions.
