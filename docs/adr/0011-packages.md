# ADR 0011: Packages and the package manager

Proposed, 2026-10-21. For the founder's approval; nothing here is decided until approved.

Repo citations are `path:line` on main at ec5ecff unless a branch is named. External facts were read on 2026-10-21 by the topic's research and judge passes, and the riskiest (licences, versions, dates, GitHub limits) were checked again the same day from the dev PC with read-only web and API requests; anything not rechecked says "unverified". Nothing was built, run or spiked for this ADR, and no model ran.

## Context

The founder wants `install <thing>` to work the way `sudo apt install` does on Linux, with the local AI able to find, install, upgrade and remove software for the user, and with the least technical debt later. Today the OS cannot do any of that:

- **Nothing can be added after boot.** Ring-3 programs are a fixed table filled from Limine modules at boot (kernel/src/process/programs.rs:1-3, :16). Block devices are read-only (kernel/src/block.rs:1). Uploaded component versions live in RAM only (docs/decisions.md:98; limits at kabi/src/live.rs:203-209).
- **The OS cannot check a hash or a signature.** There is no SHA-256 or signature code in kernel, livecore, user or kabi. Only xtask hashes, with sha2 0.11.1 (Cargo.lock:817-818). The OS never checks the `build` and `source` hashes in a component's manifest.
- **The nearest thing to a package manager is on the host.** `xtask/models.json` pins each model by repo, revision, SHA-256 and size (xtask/models.json:153-156), and `fetch` keeps a download only if it matches, then renames it into place (xtask/src/models/fetch.rs:67-104). Limine is pinned the same way (xtask/src/main.rs:61-62). The trust root is the git repository. There are no releases, no tags and no workflow with secrets; every workflow has `contents: read` (.github/workflows/ci.yml:9 and the others).
- **The nearest thing to a package format is the live artifact.** A core Wasm module with `kai.manifest` and `kai.source` sections (kabi/src/live.rs:196-199). The `kai-source 1` bundle is length-framed text with plain relative paths (livecore/src/artifact.rs:156-224). A notes artifact is about 72 KiB (kabi/src/live.rs:203).
- **The shell misreads package lines** (traced in the code, not run). `install firefox` reads as plain words and goes to the model. `install foo-1.2.3` is a Rhai syntax error. `install -y foo` would be Rhai even if `install` were a command, because `-` after a name marks Rhai (shellcore/src/commands.rs:31). Bare `undo` and `show` are caught before Rhai (shellcore/src/lib.rs:251-257), which is the place package lines can be caught too.
- **The network reaches only fixed destinations.** Ring 3 sends HTTP only to `llm=` or `build=` (kernel/src/process/syscall.rs:243-249; decisions.md:71). A whole response, headers included, is buffered up to 256 KiB (llmproto/src/http.rs:7, :72). PR #16 (branch `dane/net-dhcp-dns-b5bdb7`, 131aa77) adds RESOLVE as call 37 (kabi/src/call.rs:134 there) and follows CNAMEs (netcore/src/dns.rs:178; boot case kernel/src/net_tests.rs:156 there).
- **The default model is too big for one GitHub release asset.** It is 2,497,281,120 bytes (xtask/models.json:156); a release asset must be under 2 GiB.
- **Probation and failed-version rules exist.** Probation is 16 calls and 30 s (livecore/src/settings.rs:33-34). A boot version that fails is recorded by its commit hash (`refs/kai/boot/failed/notes/<sha12>`) and never tried again (decisions.md:118). Packages should reuse both.

Neighbouring ADRs fix most of the parts a package touches. ADR 0001 ([0001-capabilities.md](0001-capabilities.md)) owns handles, the `kai-app 1` grammar and `kai-grants 1`. ADR 0002 ([0002-agent-actions.md](0002-agent-actions.md)) owns action names, tiers, consent, `actlog 1` and undo. ADR 0003 ([0003-app-model.md](0003-app-model.md)) owns the app artifact (`kai-manifest 2`, `kai.source`, `kai.actions`, `kai.app`, `kai.notices`). ADR 0004 ([0004-storage.md](0004-storage.md)) owns kaifs, its transactions and snapshots, and `/system/models/<sha256>.gguf`. ADR 0008 ([0008-updates.md](0008-updates.md)) owns the one TUF repository and root and the `kaituf` client. ADR 0009 ([0009-licence.md](0009-licence.md)) owns licence classes and notices. This ADR decides the package itself, the store, the generations, resolution, the commands and the `pkg` service.

## Options considered

Three designs were written and judged (scores out of 10).

| Design | Score | Evidence for | Evidence against | Result |
|---|---|---|---|---|
| **A. Our own signed package file and a static TUF index** | 8 | The most complete user path: aliases, `sudo` dropped with a note, "the user's typed line answers the ask". Standard TUF JSON gives one trust root with ADR 0008, a community conformance suite and standard signing tools, and the transport is never trusted. The package container reuses the `kai-source 1` grammar, so no new container parser enters the OS. About 6,000 lines and 6-9 weeks to v0.1 (its estimate). | It proposed a third (JSON) manifest that repeats `kai-app 1` and `kai-manifest 2`; a new cap bit that clashes with ADR 0007's; Ed25519 only, which tuf-on-ci's key types do not fit; a vendored no_std pubgrub fork that v0.1 does not need. About 1,800 lines of our own security code. | **Base** |
| **B. OCI artifacts in the Wasm registry ecosystem (GHCR)** | 5 | The best measured facts. An OCI layer digest is the SHA-256 we already pin: Docker Hub's `ai/qwen3:4b-instruct-2507-q4_K_M` layer is 2,497,281,120 bytes with our pin's digest (rechecked). GHCR allows 10 GB a layer. | A single-key "TUF-lite" with no threshold. An in-OS OCI client with token exchange, a 307 redirect and two hosts, yet the index is still ours. Depends on GHCR's "currently free" tier. It would rewrite decisions.md:71. wkg interop holds for components; ADR 0003 picked core modules. v0.1 is offline, so OCI buys nothing until P5. | Lost; its model-mirror findings are kept |
| **C. A content-addressed store** | 7 | The closest to the code: it extends the catalogue pins, manifest hashes, `c-<sha12>` labels and the failed-by-hash rule. Install, remove, rollback and undo are one atomic switch. No solver: CI pins closures. Grants live in generations. A crash-injection harness tests the switch. | TUF in our own line format loses the conformance suite and ADR 0008's tooling. Its model path (`/kai/store/...`) clashes with ADR 0004's `/system/models`. It decided ADR 0008's boot generations early. Chunked release objects add machinery that Hugging Face plus an OCI mirror avoid. | Its store, generations and no-solver rule are grafted |

Open-source reuse, with licences as read on 2026-10-21. Under the founder's rule of 2026-10-21, MIT, Apache-2.0, BSD, ISC and zlib crates need no approval and are listed in each PR; MPL-2.0, GPL and LGPL ask.

| Project, version | Licence | Use | Result |
|---|---|---|---|
| TUF specification 1.0.37 (2026-10-08) | specification | roles, client order, threat list, consistent snapshots | adopt the metadata model |
| ed25519-compact 2.6.0 | MIT | Ed25519 verify in ring 3; every dependency optional | adopt (free class) |
| p256 0.14.0, ecdsa 0.17.0 | Apache-2.0 OR MIT | ECDSA P-256 verify; P5's TLS needs it anyway | adopt (free class) |
| sha2 0.11.1, serde_json 1.0.154 | MIT OR Apache-2.0 | SHA-256; metadata parsing | already in Cargo.lock:804-805, :817-818 |
| tuf-conformance v2.5.0 | MIT | client test suite in CI | adopt, CI only |
| tuf-on-ci v0.21.0 | MIT (LICENSE file; GitHub's API says NOASSERTION) | signing the repository (ADR 0008's choice) | CI only |
| tuftool 0.17.0, tough 0.24.0 | MIT OR Apache-2.0 | host oracle and fixtures; tough depends on aws-lc-rs, which is C | CI only |
| python-tuf v7.0.1 | Apache-2.0 and MIT (two licence files) | interoperability oracle | CI only |
| rust-tuf (`tuf` 0.3.0-beta14, 2025-10-20) | MIT OR Apache-2.0 | a Rust client; its README says "Beta Software" | not used |
| minisign-verify 0.3.0 | MIT | one Ed25519 check | only if S11-1 fails |
| ed25519-dalek 3.0.0 | BSD-3-Clause | Ed25519 | not chosen: ed25519-compact has no required dependencies |
| pubgrub 0.4.0, version-ranges 0.1.3 | MPL-2.0 | resolver with readable failure chains | deferred until a real conflict; ask class |
| priority-queue 2.7.0 | LGPL-3.0-or-later OR MPL-2.0 | a pubgrub dependency, taken under MPL-2.0 | deferred with pubgrub |
| resolvo 0.12.2 | BSD-3-Clause | CDCL resolver; not no_std | fallback if MPL is refused |
| libsolv 0.7.40 | BSD-3-Clause | SAT resolver | no: about 2.2 MB of C (GitHub's language count) |
| Redox pkgar-core 0.2.4 | MIT | signed archive | no: a second trust path in its header and BLAKE3 beside our SHA-256 |
| warg v0.10.0 | Apache-2.0 WITH LLVM-exception | Wasm registry protocol | no: archived 2025-07-28 |
| wa.dev | service ("Public Beta", JAF Labs, Inc.) | registry | no: its `registry.json` names a warg URL |
| oci-client 0.18.0 | Apache-2.0 | OCI client | no: needs std, async, reqwest |
| ORAS v1.3.5, cosign v3.1.4 | Apache-2.0 | OCI push; B's signing | ORAS for spike S11-3 only; cosign not used |
| wkg 0.16.1 | Apache-2.0 WITH LLVM-exception | host-side Wasm registry tool | later, host side only |
| Nix 2.35.3, Guix v1.5.0 | LGPL-2.1, GPL-3.0 | generations, atomic switch, GC roots | ideas only |
| Haiku packagefs, Fuchsia pkg | MIT (most), BSD-style | "activated, not extracted"; TUF over content-addressed blobs | ideas only |
| crates.io sparse index | cargo: MIT OR Apache-2.0 | one entry per version, only `yanked` changes | rules copied, no code |
| apt 3.3.4, dpkg 1.23.11 (upstream tags; the guest runs Debian 13's own builds) | GPL-2+ | the guest's own package manager in P6 | run unchanged inside the guest only; founder question 2 |

## Decision

Kernel AI gets its own package manager. Typing `install notes` (or `apt install notes`) fetches a signed package, checks it, and switches the system to a new numbered list of installed software in one step, so `undo` puts the old list back at once. The AI uses the same commands: software you name installs at once unless it needs a new permission, and software the AI picks for you asks once first. In v0.1 installs come from the OS image and a local mirror; installs from the internet arrive with v0.2.

In detail: base design A, with C's store, generations and no-solver resolution, and B's model and mirror findings.

**1. The package file.** A `.kpkg` is `kai-pkg 1`: the `kai-source 1` grammar (`file <path> <len>`, then the bytes; plain relative paths) under its own header. The bundle reader moves from livecore/src/artifact.rs:156-224 into a no_std, host-tested crate `kaipkg`; livecore keeps calling it for `kai-source 1`. Limits: 512 files and 64 MiB; no compression (a later `kai-pkg 2` may add it). The first file is `kai-package`; for `app` and `tool` kinds the second is the ADR 0003 artifact, whose `kai.app` section carries the ADR 0001 `kai-app 1` text. Packages are not signed inside; the signed TUF target pins length and SHA-256.

**2. The header, `kai-package 1`.** Text, one key per line, fixed order, strict like `Manifest::parse` (livecore/src/artifact.rs:120-153):

```
kai-package 1
name notes
version 2.1.0
kind app
summary Plain-text notes the AI can read and write
licence MPL-2.0
abi kai:app@0.1
depends spell-en ^1.2
```

- `kind` is `app`, `tool`, `data`, `model` or `linux-layer`. `licence` is a required SPDX expression. `summary` is at most 80 bytes and is data, never instructions. `depends` lines are optional and repeatable.
- `model` and `linux-layer` packages hold no large payload. Their header adds `blob <sha256> <length>`, and the bytes come through ADR 0008's chunked download (`kai-chunks 1`, 2 MiB chunks).
- Names are `[a-z][a-z0-9-]{1,11}`: at most 12 bytes, so `<app>_<function>` fits in ADR 0002's 24 bytes. A `-` becomes `_` in action names. CI and `pkg` refuse a package whose action names would collide with an installed package's (`pdf-view` + `open` against `pdf` + `view_open`). Names also fit service names (supervise/src/lib.rs:111-116). Official names are flat; other repositories show as `repo/name`.
- Versions are SemVer `MAJOR.MINOR.PATCH`, no pre-release tags in format 1. Ranges are `^`, `~`, `=` and comparison pairs.
- There is no JSON manifest. Three formats keep three jobs: `kai-package 1` (this header), `kai-app 1` (grants and routes, ADR 0001) and `kai-manifest 2` (runtime and lifecycle, ADR 0003). CI refuses a package whose parts disagree: the name, the artifact's world and `kai-app 1` id, the action prefix, the `abi`, and the licence against `kai.notices`.

**3. Trust.** One TUF 1.0.37 repository and root for OS images, models and packages (ADR 0008), in standard JSON so tuf-on-ci or tuftool can sign it. Packages sit under a delegated role `packages`; models under `models`. The root is 2 of 3 offline keys (ADR 0008: YubiKeys, PIV P-256). The `packages` role is signed at each publish by 1 of 2 offline YubiKeys until third-party publishers arrive in P8 (founder question 4). Each target is named `<sha256>.<name>-<version>.kpkg` (consistent snapshots), and its `custom.kai` carries the index fields: name, version, kind, summary, abi, depends, grant names, action names, licence, `yanked`. `pkg` refuses a package whose own header differs from its index entry (cargo's rule). In the OS, ADR 0008's no_std `kaituf` client checks ECDSA P-256 (p256 0.14.0) for YubiKey and KMS keys, Ed25519 (ed25519-compact 2.6.0) for file keys, and SHA-256 (sha2 0.11.1). Canonical JSON goes through serde_json if ADR 0008's spike S08-3 finds it byte-identical, else through a port of olpc-cjson 0.1.4 (ADR 0008). That serde_json matches is plausible (in our Cargo.lock it has no `indexmap`, so its `Map` is a BTreeMap) but unverified. Clock: Limine's boot date (`DateAtBootRequest`, crate limine 0.6.5 at Cargo.lock:509-510, its src/request.rs:544) plus the monotonic clock, never earlier than the newest timestamp seen. Expiry periods and the trust cache are ADR 0008's.

**4. Hosting.** In ADR 0008's repository `dane1599d/kernel-ai-updates`: TUF metadata on its GitHub Pages, `.kpkg` files as its Release assets, one release per package version tagged `pkg-<name>-<version>`. A release takes up to 1000 assets, each under 2 GiB, with no total or bandwidth limit; Pages has 1 GB and a 100 GB a month soft limit (docs.github.com, rechecked 2026-10-21). Models stay on Hugging Face, pinned by revision and SHA-256, with a GHCR ModelPack mirror from P5 whose digest equals the pin. The transport is never trusted, so any mirror serving the same bytes is safe.

**5. The store and generations.**
- Objects are `/system/pkg/store/<sha256>.kpkg`, immutable and run in place (entries are length-framed, so nothing is unpacked). A download lands in `/system/pkg/store/.partial/` and is renamed in only after length and SHA-256 match, as xtask/src/models/fetch.rs:67-104 does on the host. Large `linux-layer` blobs go to `/system/pkg/blobs/<sha256>`; model blobs go to ADR 0004's `/system/models/<sha256>.gguf`.
- A generation is `/system/pkg/gen/<n>` in `kai-gen 1`. `/system/pkg/current` and `/system/pkg/good` hold generation numbers:

```
kai-gen 1
parent 41
time 2026-11-02T10:31:00Z
why actlog 4812
pkg notes 2.1.0 9f42...8b1d explicit grants 17 18
pkg spell-en 1.2.3 1c0e...77aa dep
```

- A switch writes the new generation, `current`, the changed `kai-grants 1` lines in `/system/grants` and the `actlog 1` entry in **one** ADR 0004 transaction. Rolling back a generation rolls back its grants too.
- History is append-only: `rollback k` writes generation n+1 as a copy of k, so undoing a rollback is another switch.
- GC keeps the last 10 generations and every generation from the last 30 days, then deletes objects no kept generation names. It never removes the model in use, nor the previous one until the new one has committed and one more boot has completed (ADR 0008's rule). Each GC run is logged.
- App data never lives in the store. Remove keeps the app's data folder; `purge` removes it through ADR 0004's trash and snapshots.
- Until P2's disk exists, the image's repository is generation 1 and later generations live in RAM until reboot.

**6. Resolution.** No solver in v0.1. Take the newest compatible version that is not yanked, and keep an installed version that already satisfies the range. The OS ABI and the `kai:*` WIT worlds are virtual packages, so `abi` is an ordinary dependency. A failure is one sentence in code's words: `Cannot install notes 3.0.0: it needs kai:app@0.2; this OS has 0.1.` Libraries are linked at build time (composition is done deliberately there, since `wac plug` "doesn't cover transitive dependencies"), so install-time graphs stay small. A real solver goes behind `kaipkg::resolve` when the first real conflict appears (founder question 2).

**7. Probation and failure.** Packages that changed start pending under livecore's rules (16 calls and 30 s, livecore/src/settings.rs:33-34). A trap, failed self-test or failed health check before probation ends sets `current` back to `good`, records the hash in `/system/pkg/failed`, and prints one line. A failed hash is never retried without `force`, as with boot versions (decisions.md:118). When probation ends, `good` moves to `current`.

**8. Network.**
- **v0.1:** fetches go only to a fixed `pkgrepo=IP:PORT` destination beside `llm=` and `build=`. Ring 3 still never chooses a destination (decisions.md:71 unchanged). In CI that is xtask's test-signed mirror. Fetches are 192 KiB `Range` GETs: the 256 KiB limit covers the whole response, headers included (llmproto/src/http.rs:7, :72), so a 256 KiB body would be refused as too large. Built-in apps ship as a signed repository inside the slot image (ADR 0008's `kai-bundle 1`) and become generation 1.
- **P5:** an ADR 0001 network grant to the repository hosts over TLS replaces `pkgrepo=`. Names resolve through RESOLVE (37). The hosts are the Pages host, github.com and, after the release redirect, release-assets.githubusercontent.com (a HEAD request for a public release asset from the dev PC on 2026-10-21 got a 302 there), plus Hugging Face for model bytes. Downloads move to ADR 0008's streaming path with 2 MiB chunks.

**9. Commands.** Package lines are caught before Rhai, beside `undo` and `show`, and only when the first word is not a variable in scope:

| Kernel AI | Also accepted |
|---|---|
| `install <name>[@<range>] ...` | `apt`, `apt-get`, `dnf`, `brew`, `winget` with `install`; `pacman -S` |
| `remove <name>`, `uninstall <name>` | `apt remove`, `apt-get remove`, `dnf remove`, `brew uninstall`, `winget uninstall`, `pacman -R` |
| `upgrade [<name>]` | `apt upgrade`, `dnf upgrade`, `brew upgrade` |
| `pkg search\|info\|list\|history\|rollback [n]` | `apt search`, `apt show`, `apt list --installed` |

A leading `sudo` is dropped with a one-time note ("no sudo needed: the question is the permission"). `-y` and `--yes` are accepted and ignored: the tier decides, never a flag. Bare `search` and `list` stay free for files. Replies are one code-written line: `Done: installed notes 2.1.0 (74 KB). undo`. Before P6, `apt install python3` gets `No Kernel AI package named python3. Linux packages need the Linux layer (v0.2).`

**10. Actions and tiers** (ADR 0002 rows; names snake_case, at most 24 bytes):

| Action | Tier | Inverse |
|---|---|---|
| `pkg_search`, `pkg_info`, `pkg_list`, `pkg_history` | run | none needed |
| `pkg_install` | run when the user's own line names the package and it needs no grant that ADR 0001 requires a user choice for; otherwise one code-written ask. An install the AI chose always asks. | switch to the parent generation, compare-and-set on `current` |
| `pkg_upgrade` | run when it adds no grant; otherwise ask | the same |
| `pkg_remove`, `pkg_rollback` | run | the same; rollback's undo rolls forward |
| `pkg_purge`, `pkg_gc` | ask: they destroy data or rollback points | none |
| `pkg_repo_add`, `pkg_trust_key` | never for the AI; the user types them | none |

This **departs from the brief's "install is ask tier"**. A user-named install runs because it has an exact, cheap undo (a generation switch), ADR 0002's consent rule counts the user's own line as consent, and the founder said "if i tell it to do something it should just do it". An install the AI chose still asks once, because it brings in new code the user did not name: `Install pdfview 0.3.1 (1.2 MB, MIT)? It opens files you give it; no network. y/n`. "Pull up my browser and go to youtube" with no browser installed is `pkg_search` (run), `pkg_install` (one ask), then `browser_open` (run). A package's actions join the registry, and its action group joins ADR 0010's skill pack, when its generation becomes current; they leave when it stops being current. Index text (summaries, action names) is data and can never raise a tier.

**11. Kinds beyond apps.**
- `tool` adds a terminal command through ADR 0003's command interface.
- `data` carries files only, and no grants.
- `model` names a target under ADR 0008's `models` role. Its blob lands at `/system/models/<sha256>.gguf`; which model is in use stays ADR 0008's `model_use` action, with its fluency gate and probation.
- `linux-layer` (P6) is the Debian 13 base image of ADR 0006's Linux micro-VM, pinned by SHA-256. It is a sealed blob that only seeds the guest's disks at first start and on "reset Linux"; the writable disks are user data in the kaifs volume `linux` (ADR 0006). `apt install X` with no Kernel AI package X, and the layer installed, takes an ADR 0004 `package` snapshot of that volume, runs the guest's own apt unchanged through ADR 0006's guest agent `kai-guest`, and records `foreign debian X <version>` in a new generation; `undo` restores the snapshot. Upgrading the guest is ADR 0006's `linux_upgrade`. The guest's own signatures stay its trust chain.
- Apps built in the terminal (ADR 0012, `cc build`) install from a local repository `local`, signed by an Ed25519 device key made at first use. Only the user can trust another publisher's root (`pkg_trust_key`, never the AI).
- No kind carries native code for the OS. Apps and tools are Wasm modules run by the wasmi 2.0.0 interpreter through `wasmhost` (wasmhost/src/lib.rs:1; Cargo.lock:1153-1154), so no package creates executable memory at run time before v1.0; JITs such as the browser's run only inside the Linux guest.

**12. The `pkg` service.** A supervised ring-3 service. Its logic sits in host-tested crates (`kaipkg`; `kaituf` from ADR 0008), the way editcore and livecore are split from their services. It holds ADR 0001 handles: `/system/pkg` read-write, a route to the supervisor to start and swap apps, the `pkgrepo=` exchange in v0.1 and a network grant from P5. It gets no hard-coded cap bit and no name-bound right like PUBLISH (kernel/src/process/supervision.rs:265-283).

## Consequences

- **Good.** One install path for built-in apps, extras, tools, data, models and (from v0.2) the Linux layer. One trust root with OS updates. Every format has a version header, and every change is one transaction with an exact undo. A bad version rolls back by itself. Familiar `apt` and `brew` lines work. The AI can search, install and remove through registered actions, and a typed `install notes` just happens.
- **Costs.** About 5,000-6,000 lines to v0.1 (estimate from designs A and C), including `kaipkg`, the store, generations, the service, shell parsing, xtask repository tools and boot cases. `kaituf` (about 1,800 lines by design A's estimate; S11-1 caps it at 2,000) is security code we own. TUF's running work falls on one person: a daily timestamp job and a key ceremony.
- **Limits in v0.1.** `install` reaches only the image's repository and a local mirror. Internet installs arrive with P5 in v0.2. Updates fetch whole packages; there are no deltas. At most 16 processes and 8 components (kernel/src/process/mod.rs:54; kabi/src/live.rs:192) cap how many apps run at once, not how many are installed.
- **Two kinds of rollback.** Native packages roll back per generation. Linux-layer packages roll back through a snapshot of the guest's whole `linux` volume. Users may notice the difference.
- **GitHub and Hugging Face** see users' IP addresses and what they fetch, and stay single points of failure until a second mirror exists.

## What it costs to change later

| Choice | Cheap until | After that |
|---|---|---|
| `kai-pkg 1` container | v0.2's public repository | `kai-pkg 2` beside it, served both ways for one release; old clients refuse 2 with a clear message |
| `kai-package 1` header | v0.2 | new keys need `kai-package 2`; the strict parser refuses unknown keys |
| Package names and the 12-byte rule | first public publish | names are forever; `repo/name` is reserved for others now |
| SemVer and range syntax | first public publish | expensive: every header carries ranges |
| TUF metadata and keys | always cheap | rotation is built in; leaving TUF needs one last TUF-signed update with the new root (shared with ADR 0008) |
| Hosting | always cheap | mirrors are configuration; the transport is untrusted |
| `kai-gen 1` on disk and `/system/pkg` layout | first P2 release | versioned header, migrated on upgrade; no app ever sees a store path |
| No solver in v0.1 | always cheap | a solver fits behind `kaipkg::resolve`; the index already carries ranges |
| `pkg_*` action names | v0.1 | renames break the model's learnt calls: keep aliases in the registry and rerun the fluency test |
| SHA-256 as the object name | medium | every object rehashed and renamed |

## Spikes that settle it

None runs a model. Nothing runs on the founder's PC.

**S11-1 `kaituf-0`** (about 1 week; one branch with ADR 0008's client spike, which owns the crate). Where: GitHub Actions only, host tests on an ubuntu runner plus boot case `test=tuf-bench` under KVM and TCG.
- **Build:** no_std `kaituf` with top-level roles, one level of delegation (`packages` and `models`), root rotation, consistent snapshots, ECDSA P-256 and Ed25519, SHA-256, canonical JSON as S08-3 decides; a CLIENT-CLI.md wrapper for tuf-conformance; a ring-3 `tufbench`.
- **Measure:** (1) tuf-conformance v2.5.0 results; (2) repositories made by tuftool 0.17.0, python-tuf 7.0.1 and tuf-on-ci v0.21.0 with P-256 and Ed25519 test keys are accepted, and 20 tampered variants are refused: a flipped byte per role, expiry, version rollback, below threshold, mix-and-match, oversize. tuf-on-ci's own `tests/expected/*` fixtures mix Ed25519 and ECDSA P-384 keys; a role there that needs a P-384 signature must be refused with a named "unsupported key type" error, not accepted; (3) non-test lines, and `.text` added to a release ring-3 binary; (4) time to verify a 1,000-target `packages` role, and SHA-256 throughput on 16 MiB.
- **Pass:** every conformance test passes except multi-level and hashed-bin delegations (not needed before P8); one level is tested because `packages` and `models` are delegated roles. 100% of positives accepted and 20/20 negatives refused; at most 2,000 non-test lines; at most 200 KiB of `.text` with both curves; under KVM, at most 100 ms for 1,000 targets and at least 100 MB/s of SHA-256. TCG numbers are recorded only (decisions.md:198).
- **Fail:** a security test we cannot fix, or canonical JSON that neither serde_json nor the olpc-cjson port gets byte-identical within 500 lines. Then use design C's line-format signed root, snapshot and timestamp, and keep everything else. That fallback changes ADR 0008 too.

**S11-2 `store-switch`** (about 1 week). Where: GitHub Actions only, host tests plus boot case `pkg-spike` under KVM and TCG.
- **Host harness:** 10,000 random install, remove, rollback and gc sequences, with a crash injected at every write, flush and commit. It runs over kaifs's transaction API once that exists, and over design C's three-primitive model (write-and-flush, rename-replace, remove) before then.
- **Boot case:** xtask serves a test-signed repository holding a 900 KiB component, a 2 MiB `data` package of 40 files, and a 64 KiB `data` package both depend on. `pkg` installs through 192 KiB `Range` GETs, switches, starts the component, rolls back, then installs again from the store with no fetch.
- **Pass:** 0 invariant violations (`current` names a fully present, verified closure; no object reachable from a kept generation is removed; no half-written object is visible); 5/5 tamper cases refused with named errors (a flipped object byte, an expired timestamp, an older snapshot version, a target signed by an unknown key, a header that differs from its index entry); under KVM, install at most 3 s and rollback at most 100 ms.
- **Fail:** the switch needs more than one ADR 0004 transaction. ADR 0004 must then add it before P2 closes.

**S11-3 `model-mirror`** (at P5 entry; it does not block this ADR). Where: a `workflow_dispatch` job on a GitHub ubuntu runner, which needs a workflow with `packages: write`. The model is only moved and hashed, never run.
- **Build:** push the pinned 4B as a CNCF ModelPack artifact to GHCR with pinned ORAS v1.3.5; pull it anonymously, cutting the download at 40% and 80%.
- **Pass:** 3/3 pushes under 8 min (GHCR's upload limit is 10 min); each resume re-fetches at most 8 MiB; the digest equals the pin (xtask/models.json:155); 50 anonymous pulls with no HTTP 429.
- **Fail:** Hugging Face stays the only source, and Docker Hub's `ai/qwen3:4b-instruct-2507-q4_K_M` (same digest) is the mirror.

## Phases that depend on it

| Phase | What it needs from this ADR |
|---|---|
| P1 Agent core | the `pkg_*` registry rows, the generation inverse, the consent rule for user-named installs, and the shell's package-line parser and aliases (against the image repository and RAM generations) |
| P2 Files and storage | `/system/pkg` on kaifs: the store, generations, `current`/`good`, GC, failed hashes, and the one-transaction switch (S11-2) |
| P3 Desktop | the launcher lists the current generation's apps |
| P4 Built-in apps | built-in apps as signed `.kpkg` packages in the image's repository, so the boot manifest stops being hard-wired to notes (livecore/src/boot.rs:13); install from it and from a local mirror; the model manager uses the same path; `cc build` installs from `local` (v0.1) |
| P5 Secure networking | the online repository over TLS, the network grant replacing `pkgrepo=`, chunked resumable downloads, in-OS model setup (L7, docs/roadmap.md:102), S11-3 (v0.2) |
| P6 Running existing apps | the `linux-layer` package and `apt install` routing into the guest (v0.2) |
| P7 Real hardware | the USB image carries the same image repository; nothing else |
| P8 Production | third-party publishers (delegations under `packages`, hashed bins), grants bound to the signer key hash (ADR 0001), the installer seeding the store, fuzzing of `kaipkg` and `kaituf`, a security review |

## Changes to recently merged and open work

- **PR #15, seamless actions (merged).** One small follow-up PR now, with no change in behaviour:
  - shellcore/src/lib.rs:251-257: turn the bare-word `match` into a table of first words and handlers, with a test that each is caught only when it is not a variable in scope. Why: P1 adds `install`, `remove`, `uninstall`, `upgrade`, `pkg`, `sudo` and the aliases there, and a line that starts with one must never reach `split_command` (shellcore/src/lib.rs:266), where `-` would turn it into Rhai.
  - shellcore/src/tiers.rs:16: reword `Tier::Ask`'s doc from "Runs only after a yes: it has no undo" to ADR 0002's text, "Runs only after a yes: it has no undo, the model added it, or it needs a new grant". Why: an AI-chosen install has an exact undo and still asks. This can share a PR with ADR 0003's change to `of_script`.
  - At P1, not now: the `pkg_*` rows go into ADR 0002's registry, and the interim `Inverse` gains `Generation { from: u32, to: u32 }`, which keeps it `Copy` (shellcore/src/undo.rs:14-18).
- **PR #13, parser hardening (merged).** Follow-up PR now: add `livecore/tests/hostile.rs` (for `Manifest::parse` and `source_files`) and `editcore/tests/hostile.rs` (for `editcore::wire`), including `testkit/hostile.rs` by path as the other five crates do. Today it covers only acpitables, bpe, gguf, llmproto and uelf (testkit/hostile.rs:1-2). Why: `kai-pkg 1` reuses the bundle parser for bytes from the network. `kaipkg` and `kai-gen 1` join the harness in their first PRs.
- **PR #16, DHCP/DNS/ping (open, 131aa77).** No change. RESOLVE (37) with CNAME following reaches the GitHub hosts in P5. Do not add a destination there; `pkgrepo=` comes with the `pkg` service.
- **PR #17, terminal look (merged).** No change. docs/third-party-notices.md is the precedent for each package's `kai.notices`; its OFL font follows ADR 0009.
- **PR #10 (merged).** When `pkg_*` rows enter the prompt or skill pack, rerun the M8 fluency test on GitHub runners against its 47/50 (GitHub Actions run 39675848260, cited in PR #10's last commit, bc0eef3).
- **PR #14 (merged), PR #18 virtio-gpu and PR #19 USB input (open).** No effect.
- **Docs, on approval** (sequenced by [0000-execution-plan.md](0000-execution-plan.md)): docs/roadmap.md:139 gains packages in the P0 list, and :140, :141, :143, :144 and :145 gain the P1, P2, P4, P5 and P6 items in the table above. docs/decisions.md gains this ADR's rows. The row at :71 notes `pkgrepo=` as a third fixed destination for v0.1, and :29 notes that in-OS model setup uses this download path in P5.

## Open questions for the founder

1. **Risk: installs.** A package you name ("install notes") would install at once, with undo, instead of asking as the brief said. Do you accept that? And when the AI picks a package you did not name ("pull up my browser" with none installed), should it ask once (recommended) or install without asking?
2. **Licence.**
   - Linux layer: is it acceptable to run the guest's GPL package manager (Debian's apt and dpkg) and GPL programs as separate programs inside it? This joins ADR 0006's question 1.
   - Will the official repository accept GPL or LGPL packages at all (ADR 0009's "ask" class)?
   - When a real solver is needed: approve pubgrub 0.4.0 (MPL-2.0, with priority-queue under MPL-2.0), or stay permissive with resolvo 0.12.2 (BSD-3-Clause), which is larger and not no_std?
3. **Licence: CI-only tools.** tuf-conformance (MIT), python-tuf (Apache-2.0 and MIT), tuf-on-ci (MIT) and ORAS (Apache-2.0) are non-crate tools; tuftool, a crate, needs aws-lc-rs, which builds the C library aws-lc on the CI host. None ships in the OS. Allowed?
4. **Keys.** ADR 0008 asks about the YubiKeys and a KMS account. Here: should each package publish need your YubiKey touch until others can publish (recommended), or may CI publish packages with an online key?
5. **Publishers.** Should only Kernel AI's own packages be published until v1.0? Opening it to others brings a review policy and liability.
6. **Hosting and privacy.** Do you accept depending on GitHub (Pages, Releases, later GHCR) and Hugging Face, which see users' IP addresses and what they install? Should we fund a second mirror before v1.0?
7. **Updates.** Should upgrades that add no permission install in the background, with undo, by default?
8. **v0.1 scope.** Is it acceptable that in v0.1 `install` reaches only packages on the image and a local mirror, and internet installs arrive with P5 in v0.2?

Not asked, because the founder's 2026-10-21 rule allows them; listed for the PRs that add them: ed25519-compact 2.6.0 (MIT), p256 0.14.0 and ecdsa 0.17.0 (Apache-2.0 OR MIT), olpc-cjson 0.1.4 (MIT OR Apache-2.0) only if S08-3 needs it, and minisign-verify 0.3.0 (MIT) only if S11-1 fails.

## Sources

External (read or rechecked on 2026-10-21 unless marked):
- TUF specification 1.0.37, last modified 2026-10-08: https://theupdateframework.github.io/specification/latest/
- tuf-conformance v2.5.0 (MIT, 2026-09-02): https://github.com/theupdateframework/tuf-conformance
- tuf-on-ci v0.21.0 (MIT, 2026-09-15): https://github.com/theupdateframework/tuf-on-ci, signer/src/tuf_on_ci_sign/delegate.py:230-293 at main (Ed25519 only as a "hidden test key", :262-269) and tests/expected/basic/metadata/1.root.json at v0.21.0 (one ed25519 and one ecdsa-sha2-nistp384 key)
- python-tuf v7.0.1 (2026-09-01): https://github.com/theupdateframework/python-tuf
- tough 0.24.0 and tuftool 0.17.0: https://github.com/awslabs/tough
- rust-tuf: https://github.com/theupdateframework/rust-tuf
- Crates on https://crates.io (versions, licences and dates rechecked through its API on 2026-10-21): ed25519-compact 2.6.0, olpc-cjson 0.1.4, p256 0.14.0, ecdsa 0.17.0, minisign-verify 0.3.0, ed25519-dalek 3.0.0, pubgrub 0.4.0, version-ranges 0.1.3, priority-queue 2.7.0, resolvo 0.12.2, pkgar-core 0.2.4, oci-client 0.18.0, wkg 0.16.1, limine 0.6.5 (src/request.rs:544, https://docs.rs/crate/limine/0.6.5/source/src/request.rs)
- pubgrub: https://github.com/pubgrub-rs/pubgrub; resolvo: https://github.com/prefix-dev/resolvo; pkgar: https://github.com/redox-os/pkgar
- warg, archived 2025-07-28: https://github.com/bytecodealliance/registry; wkg: https://github.com/bytecodealliance/wasm-pkg-tools
- GitHub release limits: https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases; the redirect to release-assets.githubusercontent.com seen on https://github.com/oras-project/oras/releases/download/v1.3.5/oras_1.3.5_checksums.txt
- GitHub Pages limits: https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits
- GHCR limits and billing: https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry and https://docs.github.com/en/billing/concepts/product-billing/github-packages
- CNCF Wasm OCI artifact layout v0: https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/
- ORAS v1.3.5 (2026-10-19): https://github.com/oras-project/oras; cosign v3.1.4 (2026-10-20): https://github.com/sigstore/cosign
- apt 3.3.4 (tag 2026-10-19): https://salsa.debian.org/apt-team/apt; dpkg 1.23.11 (tag 2026-09-09): https://salsa.debian.org/dpkg-team/dpkg; libsolv 0.7.40: https://github.com/openSUSE/libsolv; Guix v1.5.0 (2026-01-21): https://codeberg.org/guix/guix
- crates.io index format: https://doc.rust-lang.org/cargo/reference/registry-index.html
- Nix 2.35 profiles: https://nix.dev/manual/nix/latest/package-management/profiles
- Haiku packages: https://www.haiku-os.org/docs/develop/packages/Infrastructure.html
- Fuchsia packages: https://fuchsia.dev/docs/get-started/learn/intro/packages
- Component composition ("The `plug` syntax doesn't cover transitive dependencies"): https://component-model.bytecodealliance.org/composing-and-distributing/composing.html
- PyPI's removal of PGP signatures, 2023-05-23: https://blog.pypi.org/posts/2023-05-23-removing-pgp/

Repository (main at ec5ecff unless marked): kabi/src/live.rs:192, :196-199, :203-209; livecore/src/artifact.rs:120-153, :156-224; livecore/src/settings.rs:33-34; livecore/src/boot.rs:13; llmproto/src/http.rs:7, :72; kernel/src/process/syscall.rs:243-249; kernel/src/process/supervision.rs:265-283; kernel/src/process/programs.rs:1-3, :16; kernel/src/process/mod.rs:54; kernel/src/block.rs:1; shellcore/src/commands.rs:31; shellcore/src/lib.rs:251-257, :266; shellcore/src/tiers.rs:16; shellcore/src/undo.rs:14-18; supervise/src/lib.rs:111-116; testkit/hostile.rs:1-2; xtask/models.json:153-156; xtask/src/models/fetch.rs:67-104; xtask/src/main.rs:61-62; Cargo.lock:509-510, :804-805, :817-818, :1153-1154; wasmhost/src/lib.rs:1; .github/workflows/ci.yml:9; docs/decisions.md:29, :71, :98, :118, :198; docs/roadmap.md:102, :139-145; docs/third-party-notices.md. Branch `dane/net-dhcp-dns-b5bdb7` at 131aa77: kabi/src/call.rs:134, netcore/src/dns.rs:178, kernel/src/net_tests.rs:156. PR #10's commit bc0eef3 (fluency 47/50). P0 inputs: D:/kai-targets/p0/work/decision-sheet.md and work/11/{judgement,design-A,design-B,design-C,research-code,research-prior}.md.
