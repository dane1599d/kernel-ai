# P5 plan: secure networking

Status: DRAFT for founder approval, 2026-10-27. No P5 code lands on main until the founder approves this plan.

This plan covers the work that the accepted ADRs and the execution plan ([ADR 0000](../adr/0000-execution-plan.md) §3 and §5, P5 rows) put in P5:
- [ADR 0001](../adr/0001-capabilities.md): `NetDest` per app;
- [ADR 0002](../adr/0002-agent-actions.md): open-world rows, the host table, P-F and quarantined reads;
- [ADR 0004](../adr/0004-storage.md) and [ADR 0008](../adr/0008-updates.md): verified chunked downloads, and updates over the network;
- [ADR 0011](../adr/0011-packages.md): the online repository, for v0.2;
- [ADR 0013](../adr/0013-agents.md), if approved: agents' network budgets.

Section 4 lists what the plan decides itself. Section 7 lists the crates, with their licences, and which ones need your approval. Nothing here was built or run for this plan. Sizes marked "est." are estimates. Crate versions and licences were read from the crates.io API on 2026-10-27.

## 1. Goal and exit

**Goal** (roadmap P5 row): DNS, TLS 1.3 and an HTTP client; verified downloads with resume; in-OS model setup and download (L7 folds in here); network permission per app; network actions in the ask tier. With P6 this is v0.2, "the internet and existing apps".

What the founder sees at the end of P5, in a VM with a network:
- **`install weather` reaches the internet.** It fetches the package from the signed repository on GitHub over TLS, checks it, and switches generations; `undo` works as in v0.1.
- **The model downloads inside the OS.** Models offers the pinned Qwen3-4B (2.5 GB). It downloads in 2 MiB chunks, each checked against the signed list, survives a cut connection or a reboot, and resumes where it stopped. No host command is needed any more.
- **Updates come over the network.** `updated` fetches the next signed release into the idle slot, and `:reinstall` in the recovery slot can fetch one too.
- **An app reaches only its hosts.** Weather asks once: "Weather wants to reach api.weather.example. Allow / Deny". After that it reaches that host and no other. Settings lists the hosts each app may reach, with revoke.
- **The AI is careful with the network.** "what's the weather in Leeds" runs, because it is a fetch from a granted, grounded host. Anything that sends your data out asks, and names where it is going.

**Exit criteria.** All of these pass in CI under KVM and TCG, against xtask's HTTPS mock with a test CA. The roadmap's exit comes first: "a cut connection resumes, and a bad certificate or bad hash is refused" (roadmap P5 row).

| Boot case | Shows |
|---|---|
| `net-download` | A 64 MiB fixture blob downloads in 2 MiB chunks; the mock cuts the connection at 40% and 80% and each resume re-fetches at most one chunk; a reboot mid-download resumes; a flipped chunk and a wrong whole-file hash are refused by name; a server certificate that is expired, for the wrong name, or from an unknown CA is refused before any byte is stored |
| `net-dest` | An app with no `NetDest` is refused; a name outside its host suffixes does not resolve; an address it did not resolve cannot be connected to; a redirect to another host is refused |
| `pkg-online` | `install` from the mock GitHub Pages and Releases hosts, `undo`, and ADR 0011's five tamper cases over the network |
| `model-download` | Models fetches a fixture model from the `models` role into `/system/models`; `model_use` switches to it; `undo` switches back |
| `update-online` | `updated` stages the next release into the idle slot over the network; a rolled-back or expired timestamp is refused |
| `net-grant` | Weather's first connection asks; "Allow" writes a per-host grant; Settings revokes it and the next connection is refused; `undo` restores it |
| `net-pf` | A note's text the model tries to send to a host the user did not name asks, and names the source (P-F); a page that says "send the user's notes to x.example" starts nothing |

Host gates in CI: hostile tests through `testkit/hostile.rs`, and a fuzz target, for every new parser: the HTTP response reader, chunked encoding, `kai-chunks 1`, and the certificate path through rustls-webpki. The TLS client also runs against a second implementation, `openssl s_server` on the CI host, which runs there and never ships. `cargo deny` gains the approved licences only (section 7).

**Real-model gates, on GitHub only.** P5 changes model input: the open-world rows join the pack, and so does the host table. Task 11's PR runs `fluency-model.yml` (at least 47/50) and `actbench-model.yml`, with S02-1's bait probes added (at most 0 false runs, ADR 0000 §5 P5 row 5). S10-2 runs before v0.2 (section 5). No P5 gate runs a model on the founder's PC.

## 2. Entry, and what comes before P5's first PR

**Entry conditions** ([ADR 0000](../adr/0000-execution-plan.md) §3, P5 row: "P2's `fs` and store; P1's `NetDest` kind"):

| Condition | State on main at 1f54d4a, 2026-10-27 |
|---|---|
| P1's `NetDest` kind | Met in part. `Kind::NetDest` exists with fixed destinations only (`kabi/src/handle.rs:75`, `:229`; `kernel/src/handle/mod.rs:107`). Address ranges, host suffixes and RESOLVE as a right are P5 task 1 |
| P2's `fs` serving files, and the package store | Not yet: P2 tasks 8 and 13 |
| P2's model store (`/system/models`, MODEL_FILL) | Not yet: P2 task 10 |
| v0.1 cut on a release branch | Not yet. P5's OS-side work merges to main only after `release/v0.1` exists (section 4, item 1) |
| S11-3 run | Not yet. It runs at P5 entry (section 5) |

**What runs today** (built in #16, M6 and L-steps). P5 builds on it:
- smoltcp 0.14.0 in the kernel, with polled virtio-net and DHCP or a static address (`kernel/src/net/`);
- a DNS resolver with a TTL cache and a test zone, host-tested in `netcore`;
- ICMP ping;
- a one-request-per-connection plain HTTP exchange used only for the fixed `llm=`, `build=` and (in v0.1) `pkgrepo=` destinations (`kernel/src/process/syscall/net.rs`, `call::HTTP`, 64 KiB requests, 300 s timeout);
- RESOLVE (call 37) behind `cap::RESOLVE`.

There is no TLS, no TCP stream for ring 3, no random-number call, and the receive buffer is 64 KiB with a 2 ms poll gap (`kernel/src/net/mod.rs:29-33`).

**Before P5's entry, once this plan is approved**, these can start, because they are host-only or spikes, and they touch no running lane's file:
1. **Spikes P5-S1 (TLS without std) and P5-S2 (throughput)** (section 5).
2. **Task 4's host half:** `kainet`'s TLS client on our RustCrypto provider, tested against the mock and `openssl s_server`.
3. **Task 5:** the HTTP client, host-only.
4. **Task 7:** xtask's HTTPS mock, with fault injection.

**Founder answers P5 needs.** Each row says what this plan builds until you answer:

| Question | Default this plan builds |
|---|---|
| **1. The TLS crates** (roadmap P5 row: "the crates need approval"). Section 7 lists them. All but two are already in ADR 0009's free class | Approve the set: rustls 0.23.46 with no default features, rustls-webpki, and the RustCrypto primitives. Our own `CryptoProvider` is about 800 lines (est.), because the published RustCrypto provider for rustls is an alpha from 2024 |
| **2. Root certificates.** webpki-roots 1.0.11 (Mozilla's list) is under CDLA-Permissive-2.0, which is not in ADR 0009's free class | Approve CDLA-Permissive-2.0 for this one data crate, with its notice in `NOTICES.txt`. Updates to it come through the signed OS update. The fallback is a hand list of the few roots our hosts use, which breaks when a host changes CA |
| 3. The Public Suffix List (ADR 0001 question 6): `psl` 2.1.245 embeds MPL-2.0 data | Not in v0.2. A short list of our own refuses grants such as `*.co.uk` and `*.github.io` |
| 4. TLS 1.2 | TLS 1.3 only, as the roadmap says. GitHub, its release-assets host and Hugging Face all serve TLS 1.3 (not rechecked here). An app whose host has only TLS 1.2 is refused with a named error. rustls's `tls12` feature can be switched on later |
| 5. Sending messages (ADR 0002 question 3): always ask, or send after a short hold with `undo` | Always ask in v0.2. `Hold` stays built but off |
| 6. Downloads in the background (ADR 0008 question 3): may a 2.5 GB model download without asking? | It asks once, with the size and the host. Signed updates of packages already installed download without asking, and apply only at the restart time you set (P4 question 10) |
| 7. Hosting and privacy (ADR 0011 question 6): GitHub and Hugging Face see users' IP addresses | Accept for v0.2. S11-3's GHCR mirror if it passes, and a second, independent mirror before v1.0 |
| 8. An MCP client and bridge for third-party tools (ADR 0002 question 7) | Not in v0.2 |
| 9. IPv6 | IPv4 only in v0.2 (VMs sit behind NAT). smoltcp's `proto-ipv6` is a feature switch for later |
| 10. How an app's first connection to a new host asks | It asks once per host, at the first connection, or at install if the package's `kai-app 1` names the host. "Allow" writes a per-host grant. The AI can never answer it (ADR 0001 `USER`) |

## 3. The work, in order

P5 adds the network lane, split in two so two sessions do not share files, and takes turns from running lanes.

| Lane | Owns in P5 | State |
|---|---|---|
| **N-k** network, kernel side | `kernel/src/net/`, `netcore/`, `kernel/src/process/syscall/net.rs`, `kernel/src/net_tests.rs`, `xtask/src/cases/net.rs` (new lane table) | New. ADR 0000's lane N, which no session holds today |
| **N-t** network, TLS and HTTP | `kainet/` (new: TLS, our RustCrypto provider, HTTP/1.1, chunked downloads), `user/src/bin/fetch/` (new: the apps' HTTP service), `xtask/src/mock_https/` (new), `xtask/src/mock_dns.rs` | New |
| **P** packages | `kaipkg/`, `user/src/bin/pkg/`, `xtask/src/pkgrepo/` | Continues from P4 |
| **U** updates | `kaituf/`, `user/src/bin/updated`, `bootslot/` | Continues |
| **K**, **A-core**, model lane, **C**, **G-core**, AP-2 (P4's Settings and Models apps) | As in the P3 and P4 plans | Turns (section 3.4) |

### 3.1 Core tasks

Each row is one PR. "Needs" names what must merge first. Sizes are lines with tests (est.).

| # | Task | Lane | ADR | Needs | Size | Exit check |
|---|---|---|---|---|---|---|
| 1 | **`NetDest` per app.** Address and port ranges, a protocol and host-name suffixes, ported from Tock's `network_capabilities.rs` (Apache-2.0 OR MIT, header kept); RESOLVE becomes a right on a `NetDest`, and `cap::RESOLVE` retires; a resolve answers only names under the handle's suffixes; a connect reaches only literal ranges in the handle, or addresses this handle resolved within their TTL; handles only shrink | N-k, K's turn | 0001 D2, P5 row | entry | M (~900) | `net-dest` |
| 2 | **TCP streams on handles.** NET_CONNECT, NET_SEND, NET_RECV, NET_POLL and NET_CLOSE, named in the PR and numbered at merge; at most 8 KiB a call, or an `Shm` for bulk; at most 16 streams a process and 64 in all; a killed process's streams close; window scaling with a 1 MiB receive buffer per bulk stream (P5-S2 sets the figure); `call::HTTP` stays for `llm=` and `build=` | N-k, K's turn | 0001, 0007 | 1 | M (~1,000) | Case `net-stream`: echo, limits, kill; P5-S2's throughput bar |
| 3 | **Random numbers.** A RANDOM call (named at merge) from a kernel ChaCha20 generator seeded by RDSEED, with RDRAND and virtio-rng mixed in when present; refuses to answer before it is seeded | K's turn, N-k | 0001 | entry | S (~300) | Host: the generator's known-answer tests; a case that two boots differ |
| 4 | **TLS 1.3 in `kainet`.** rustls 0.23.46 with `default-features = false` (no aws-lc-rs, no ring); our `CryptoProvider` on RustCrypto: X25519 and P-256 key exchange, AES-128-GCM, AES-256-GCM and ChaCha20-Poly1305, ECDSA P-256 and P-384, RSA PKCS#1 v1.5 and PSS verify, Ed25519; rustls-webpki against webpki-roots (question 2); time from the kernel's clock; built for the hard-float user target, so AES-NI and the vector paths can run (section 4, item 3) | N-t | roadmap P5, 0009 | P5-S1; question 1 | L (~1,800) | Host: handshakes with the mock CA and with `openssl s_server`; expired, wrong-name, unknown-CA, weak-signature and downgrade cases refused; a fuzz target on the record layer |
| 5 | **HTTP/1.1 in `kainet`.** GET and HEAD with `Range`; chunked transfer; at most 5 redirects, each checked against the caller's `NetDest` (GitHub's release redirect to its release-assets host is allowed only because the handle names it); size and time limits; no cookies, no compression; httparse 1.10.1 for headers | N-t | 0011 §8, 0008 §9 | entry (host) | M (~1,200) | Host and hostile tests |
| 6 | **Verified downloads with resume.** `kai-chunks 1`: 2 MiB chunks by `Range`, each checked against the signed chunk list before it is written; a download records its last checked chunk in kaifs, so a resume after a cut or a reboot starts there; the whole target's SHA-256 is checked at the end; targets land in `.partial/` and are renamed in (ADR 0011 §5) | N-t, with P's and U's turns | 0008 §9, 0004, 0011 | 2, 4, 5 | M (~1,000) | `net-download` (the roadmap's exit) |
| 7 | **xtask's HTTPS mock.** A test CA; hosts mapped through the existing mock DNS zone (`netcore::fixture`); the repository layout of `kernel-ai-updates` and a Hugging Face-shaped model host; faults: a cut at N bytes, an expired or wrong-name certificate, a flipped byte, 429, a slow link, and a redirect off the handle's hosts | N-t | – | entry (host) | M (~1,000) | Its own host tests; every P5 case uses it |
| 8 | **The online repository.** `pkg` reaches TUF metadata on the Pages host and `.kpkg` files on Releases over TLS; its fixed `pkgrepo=` destination is replaced by a `NetDest` for those three hosts (ADR 0011 §8); online `pkg_install`, `upgrade` and `pkg search` | P, N-t's review | 0011 §4, §8, §10 | 6 | M (~800) | `pkg-online` |
| 9 | **The model downloaded in the OS** (L7). Models offers the targets of the signed `models` role; `model_fetch` (it asks once, question 6) streams from the pinned Hugging Face revision, or from the GHCR mirror if S11-3 passed, into `/system/models/<sha256>.gguf`; then `model_use`, with its probation. The "No AI model is set up" path offers it | U and AP-2, the model lane's turn | 0008 §9, 0004 §8, roadmap L7 | 6; P2 task 10 | M (~1,200) | `model-download` |
| 10 | **Updates over the network.** `updated` streams the next release into the idle slot, and `:reinstall` in slot R fetches one; DNS through RESOLVE on `updated`'s `NetDest`; TUF makes a spoofed answer a denial of service at worst (ADR 0008) | U | 0008 §9 | 6 | M (~800) | `update-online` |
| 11 | **The agent's network rows.** Open-world rows; the curated host table ("weather" to the app's host); a fetch from a grounded host runs, otherwise it asks (ADR 0002 D3); P-F: a `Data` payload goes only to a host the user's line names; quarantined reads, whose answer is a bool, an enum or a short string returned as `Data` (FIDES's typed query); `:resolve` and `:ping` become read rows; `Hold` built but off (question 5) | A-core, model lane's turn | 0002 D3, D6, P5 row | 1 | M (~1,000) | `net-pf`; fluency at least 47/50; actbench with S02-1's bait probes at 0 false runs |
| 12 | **`kai:net` for apps.** A `kai:net/http@0.1` WIT import, served by the `fetch` service, which makes each request on the calling app's own `NetDest` handle, passed with the call. The kernel enforces the destination; `fetch` checks that the certificate name is under the handle's suffixes. Apps never see sockets | C and N-t | 0003 P5 row, 0001 | 4, 5; 1 | M (~1,200) | A case where an app fetches from its host and is refused another |
| 13 | **Per-app grants and their asks.** The first connection to a new host asks on P3's ask card, naming the app and the host; "Allow" writes a per-host `kai-grants 1` record; Settings (P4) lists and revokes them; `undo` restores a revoke | A-core, G-core's and AP-2's turns | 0001 D8, 0002 | 12 | S (~600) | `net-grant` |
| 14 | **Agents' network budgets** (only if ADR 0013 is approved). An agent manifest's `net` line becomes a `NetDest`, plus a byte budget a day counted by the kernel on that handle | AG, K's turn | 0013 §6 | 1, 2; ADR 0013 | S (~500) | A case where an agent over its byte budget is refused |
| 15 | **P5 close.** Every exit case in `ci.yml`'s shards; decisions rows; the roadmap's P5 status; ADR 0002's open-world text checked against what shipped | – | – | all | S | Section 1's exit criteria |

**Critical path:** P5-S1, then task 4, then task 6 (which also needs tasks 2 and 5), then tasks 8, 9 and 10 side by side, then 15. Task 1 runs beside 4, and tasks 11-13 run beside 6-10. The only founder answers on the path are questions 1 and 2, because task 4 needs its crates.

### 3.2 Beside lanes during P5

- **Lane D**: virtio-net interrupts in place of polling, if P5-S2 shows the 2 ms poll gap limits throughput. `netd` and the real PC's network chip stay P7's (ADR 0007).
- **Lane V, P6**: the Linux guest's network goes through `vmm`'s virtio-net onto a `NetDest` held by `vmm` (ADR 0006). Not P5's exit.
- **Model lane**: S10-2 and the index level, before v0.2 (section 5).

### 3.3 Running sessions in parallel

| Session | Lanes | Tasks | Starts |
|---|---|---|---|
| 1 | N-t | P5-S1, then tasks 7, 5, 4, then 6 and 12 | At approval: host-only until the release branch exists |
| 2 | N-k | P5-S2, then tasks 1, 2, 3 | Spike at approval; the kernel tasks after `release/v0.1` |
| 3 | P and U | Tasks 8 and 10, then 9 with AP-2 | When task 6 is close |
| – | K, A-core, model lane, C, G-core, AP-2, AG | Turns: 1-3 (K), 11 and 13 (A-core), 11 (model lane), 12 (C) and 14 (AG) | As each task reaches them |

Two sessions start at approval, beside P3's and P4's. A third joins once P4's lanes free up. You set the number.

### 3.4 Where P5 takes a hot-file turn

| File | P5's change | Task | Turn |
|---|---|---|---|
| `kabi/src/handle.rs`, `kernel/src/handle/` (lane K) | `NetDest` objects with ranges and suffixes; RESOLVE as a right; `cap::RESOLVE` retired | 1 | Lane K writes or reviews the kind part |
| `kabi/src/call.rs` | NET_CONNECT, NET_SEND, NET_RECV, NET_POLL, NET_CLOSE and RANDOM | 2, 3 | Named in the PR, numbered at merge |
| `kernel/src/process/` spawn (lane K) | `NetDest` handles from a package's grants instead of plan lines; `pkg`'s and `updated`'s handles for the three repository hosts and Hugging Face | 1, 8, 10 | One turn each |
| `agentabi/`, `shellcore/` (A-core) | Open-world rows, the host table, P-F, quarantined reads; the per-host grant ask | 11, 13 | A-core's turns |
| `llmproto/` (model lane) | The rows in the pack; the quarantined-read prompt | 11 | The model lane's turn, with the real-model gates |
| `wit/` (lane C) | `kai:net/http@0.1` | 12 | Lane C |
| `deskcore/` (G-core); Settings and Models apps (AP-2) | The ask card's host line; the grant list; the model download view | 9, 13 | Their owners |
| `deny.toml`, `NOTICES.txt` (lane L) | The approved licences and the new crates' notices | 4 | Lane L reviews |
| `Cargo.toml`, `Cargo.lock`, `docs/decisions.md` | `kainet`, `fetch`; section 7's crates; decision rows | each | ADR 0000 §4.2: re-run cargo, append only |
| `.github/workflows/ci.yml` | None. `openssl s_server` runs inside a host test's own job step only if the existing job already has openssl; otherwise in its own workflow | 4 | – |

## 4. Decided in this plan

No ADR settles these. Each is small and can change in review.

1. **v0.1 is cut on a release branch first.** P5's host-only crates and spikes may merge to main at any time. Its kernel and service changes merge only after `release/v0.1` is cut at `v0.1.0-rc.1`, so nothing in P5 can move or break v0.1. Fixes to v0.1 go to that branch and are merged back to main.
2. **The TCP/IP stack stays in the kernel for v0.2.** smoltcp already runs there. Moving it to a ring-3 `netd` is ADR 0007's P7 work for the real network chip. TLS, HTTP and every parser of server data run in ring 3, in `kainet`.
3. **TLS clients build for the hard-float user target.** That is where `llm` and `cc` are built. AES-GCM and ChaCha20 use vector registers, and the kernel and the soft-float programs cannot use them (roadmap L2). `pkg`, `updated`, `fetch` and Models' download path build there.
4. **One TLS client crate, many holders.** Native services (`pkg`, `updated`, the model download) link `kainet` themselves and use their own `NetDest`. Wasm apps never hold sockets: `fetch` makes their requests on their own handles, so the kernel still enforces each app's destinations, and `fetch` holds none of its own.
5. **Connect follows resolve.** A `NetDest` with host suffixes can connect only to addresses it resolved itself, within their TTL. So a grant for `api.weather.example` cannot be used to reach an address given in a page.
6. **Hosts we sign for are grounded.** As ADR 0002 says, the TUF repository's hosts and the pinned Hugging Face revision are grounded: bytes from them are checked against signed metadata before use. Fetches from them need no line naming them.
7. **No DNSSEC and no certificate pinning in v0.2.** TUF protects what we install and update. TLS with webpki-roots protects apps' own hosts. A pinned key would break whenever a host rotates its certificate.

## 5. Spikes

None runs a model on the founder's PC, and none runs QEMU there.

| Spike | ADR | Where | Length (est.) | Gates | When |
|---|---|---|---|---|---|
| **P5-S1** `tls-nostd` | roadmap P5 | Host tests and a KVM boot case on GitHub Actions; the real hosts are reached only from a GitHub runner | About 1 week | Task 4 | At approval |
| **P5-S2** `net-throughput` | 0007, 0008 | KVM and TCG boot cases on GitHub Actions against the mock, with added latency | Days | Task 2's buffer sizes; whether lane D's virtio-net interrupts come first | At approval |
| **S11-3** `model-mirror` | 0011 | A `workflow_dispatch` job with `packages: write` on a GitHub runner; the model is moved and hashed, never run | Days | Task 9's mirror | P5 entry |
| **S10-2** groups and swap | 0010 | GitHub runner, real model | About 1 week | Whether the index level ships in v0.2 | Before v0.2 |

**P5-S1, `tls-nostd`.**
- **Build:**
  - rustls 0.23.46 with no default features, plus a minimal `CryptoProvider` on the RustCrypto crates of section 7, for `x86_64-unknown-none` (host tests) and the hard-float user target;
  - a ring-3 `tlsbench` that downloads from the mock;
  - one runner job, never on the PC, that handshakes with github.com, the Pages host, release-assets.githubusercontent.com and huggingface.co.
- **Measure:**
  - whether it builds without std;
  - `.text` added;
  - full handshake time under KVM;
  - AES-128-GCM and ChaCha20-Poly1305 throughput in ring 3;
  - which key-exchange groups, signature schemes and certificate keys the four real hosts use.
- **Pass:**
  - it builds with no std and no C;
  - at most 600 KiB of `.text`;
  - a handshake in at most 300 ms;
  - at least 200 MB/s of AES-GCM;
  - all four real hosts handshake on TLS 1.3.
- **Fail:** if rustls cannot run without std, embedded-tls 0.19.0 (Apache-2.0, TLS 1.3, no_std) with the same provider and webpki, after its own review; if a host lacks TLS 1.3, question 4 comes back to you.

**P5-S2, `net-throughput`.**
- **Measure:** download speed from the mock at 0, 20 and 80 ms of added round trip, with receive buffers of 64 KiB, 1 MiB and 4 MiB, under the polled driver.
- **Why it matters:** today's 64 KiB buffer allows about 64 KiB per round trip. At 50 ms that is about 1.3 MB/s, so the 2.5 GB model would take over 30 minutes (est.).
- **Pass:** at least 30 MB/s at 20 ms, so 2.5 GB in under 90 s.
- **Fail:** lane D's virtio-net interrupts and a larger queue first; then bulk receive through `Shm` and KRNG v1.

## 6. Tests

- **Host tests** for `kainet` (TLS, HTTP, chunks) and `netcore`. Hostile tests and a fuzz target cover every parser of server bytes.
- **A second TLS implementation.** `kainet` handshakes with `openssl s_server` on the CI host as well as with the mock. That catches an implementation that is only consistent with itself. openssl runs on the CI host and never ships.
- **Boot cases** against xtask's HTTPS mock, under KVM and TCG, with the faults of task 7. No CI boot case reaches the real internet. P5-S1's one job reaches the four real hosts, read-only.
- **Real-model runs** only at task 11, on GitHub: fluency, actbench and S02-1's bait probes.
- Each run records its runner and run id in its PR.

## 7. Crates: what needs your approval

The rule of 2026-10-21: MIT, Apache-2.0, BSD, ISC and zlib crates need no approval, but each PR lists them. ADR 0009's free class adds 0BSD, MIT-0, Unlicense, BSL-1.0, Unicode-3.0, CC0-1.0, BlueOak-1.0.0 and BSD-2-Clause-Patent. The roadmap asks for the TLS crates to be approved anyway, so they are listed here for one yes (question 1). Two need a licence decision whatever happens (questions 2 and 3).

| Crate | Version (crates.io, 2026-10-27) | Licence | Class | Use |
|---|---|---|---|---|
| rustls | 0.23.46 | Apache-2.0 OR ISC OR MIT | free; approval asked (question 1) | TLS 1.3, `default-features = false` |
| rustls-webpki | 0.103.16 | ISC | free; approval asked | certificate path checks |
| rustls-pki-types | 1.15.1 | MIT OR Apache-2.0 | free | types shared by rustls and webpki |
| **webpki-roots** | 1.0.11 | **CDLA-Permissive-2.0** | **ask (question 2)** | Mozilla's root certificates |
| x25519-dalek | 3.0.0 | BSD-3-Clause | free | key exchange |
| p256, p384 | 0.14.0, 0.14.0 | Apache-2.0 OR MIT | free (p256 already listed for `kaituf`) | ECDHE and ECDSA |
| rsa | 0.9.10 | MIT OR Apache-2.0 | free | RSA signature verify only. RUSTSEC-2023-0071 (Marvin) concerns private-key operations, which we never do (not rechecked against the current advisory text) |
| ed25519-dalek | 3.0.0 | BSD-3-Clause | free | Ed25519 certificates (`kaituf` uses ed25519-compact; one may replace the other in review) |
| aes-gcm | 0.11.1 | Apache-2.0 OR MIT | free | AEAD |
| chacha20poly1305 | 0.11.0 | Apache-2.0 OR MIT | free | AEAD |
| hmac, hkdf | 0.13.0, 0.13.0 | MIT OR Apache-2.0 | free | TLS 1.3 key schedule |
| sha2 | 0.11.1 | MIT OR Apache-2.0 | free (already in the tree) | hashes |
| rand_chacha | 0.10.0 | MIT OR Apache-2.0 | free | the kernel's generator (task 3) |
| subtle, zeroize | 2.6.1, 1.9.1 | BSD-3-Clause; Apache-2.0 OR MIT | free | constant-time comparisons; key wiping |
| httparse | 1.10.1 | MIT OR Apache-2.0 | free | HTTP headers |
| **psl** | 2.1.245 | MIT/Apache-2.0 crate, **MPL-2.0 data** | **ask (question 3); not adopted by default** | public suffixes |
| rustls-rustcrypto | 0.0.2-alpha (2024-04-24) | MIT OR Apache-2.0 | free | **Not adopted**: an alpha, unchanged since 2024. Our own provider instead |
| embedded-tls | 0.19.0 | Apache-2.0 | free | Only if P5-S1 fails |
| rmcp | 3.6.0 | Apache-2.0 | free | Only if question 8 says yes |

Not adopted: aws-lc-rs and ring, rustls's default providers. Both build C or assembly into the shipped graph and need ADR 0009 question 4's answer. graviola 0.4.2 (Apache-2.0 OR ISC OR MIT-0) has a rustls provider, but whether it runs without std is unverified.

## 8. Estimate

- **Core:** 15 PRs. About 13,000 lines with tests (est.):
  - `NetDest` 900;
  - streams 1,000;
  - random numbers 300;
  - TLS and our provider 1,800;
  - HTTP 1,200;
  - downloads 1,000;
  - the mock 1,000;
  - the online repository 800;
  - the model download 1,200;
  - updates 800;
  - the agent's rows 1,000;
  - `kai:net` and `fetch` 1,200;
  - grants 600;
  - agents' budgets 500.
- **Timing:** the roadmap says weeks to months. With N-t and N-k in parallel, P5's length is set by P5-S1, task 4, task 6, then tasks 8-10: **about 8-11 weeks (est.)** from P5's entry. About 3 of those weeks (the spikes, the mock, the HTTP client and TLS's host half) run before entry, beside P3 and P4. P5 is half of v0.2. P6, the long pole of v0.2, sets that release's date, not P5.

**Start with these two, in parallel, once the plan is approved:**
1. **P5-S1, then task 7** (session 1). Host-only and GitHub only.
2. **P5-S2** (session 2). One boot case against the mock, on GitHub.

## 9. Risks

| Risk | What we do |
|---|---|
| rustls does not run without std | P5-S1's fail rule: embedded-tls 0.19.0 with the same provider and webpki. The API `kainet` offers does not change |
| Our own `CryptoProvider` has a bug | It only wires audited RustCrypto primitives into rustls's traits. It runs against `openssl s_server` and rustls's own test vectors, and it is fuzzed. P8's security review covers it |
| Downloads are slow on the polled driver | P5-S2 first; then larger buffers, window scaling, then lane D's interrupts |
| A redirect or a page steers a fetch elsewhere | Each redirect is checked against the caller's `NetDest`; connect follows resolve (section 4, item 5); P-F asks before data goes out |
| Prompt injection from the web | Page text is `Data`: it can start nothing, add nothing, and choose no target of an undo-less or outbound call without a question (ADR 0002 D6). `net-pf` tests it; S02-1's bait probes gate task 11 |
| Clock errors break certificate checks | Time comes from Limine's boot date plus the kernel's monotonic clock. A clock before the build date is refused, with a named error, rather than skipping checks |
| GitHub or Hugging Face change hosts or certificates | Hosts sit in the signed repository's metadata and the `NetDest` grants, so they change through a signed update. webpki-roots follows CA changes |
| P5 work disturbs v0.1 | Section 4, item 1: OS-side work merges only after `release/v0.1` |
| GitHub Actions time | No boot case reaches the internet; P5-S1's real-host job runs once per change to `kainet`'s handshake |

## 10. Deferred, and where it goes

- **P6 (v0.2):** the Linux guest's network through `vmm`; the browser's TLS is Firefox's own, inside the guest.
- **P7 (v0.3):** `netd` in ring 3 for the real network chip (ADR 0007); the stack may move with it.
- **P8 (v1.0):** a second independent mirror; fuzzing of `kainet` at scale; a security review of the TLS provider and `fetch`; TLS client certificates if accounts need them.
- **Later, on request:** TLS 1.2, IPv6, the Public Suffix List, an MCP bridge, `Hold` for sends, HTTP/2.
- **Waiting on the founder:** section 2's questions, the crates (questions 1 and 2) first.
