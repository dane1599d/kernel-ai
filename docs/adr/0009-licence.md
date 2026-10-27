# ADR 0009: Release, licence, contribution and reuse terms

Accepted 2026-10-22 (proposed 2026-10-21), except the OS licence.

- **Accepted:** the reuse policy and licence classes (D4, with the founder's 2026-10-22 answers), headers and third-party notices (D5-D6), the sign-off and AI-assistance rules (D2's DCO and D3, which apply once outside contributions open), and the release terms (D7 onward).
- **Deferred:** the OS licence choice (D1, and D2's inbound-equals-outbound clause that rests on it). It is decided before v0.1. Until then the repository is "all rights reserved" (`README.md:107-109` stays) and no outside contributions are accepted.
- Questions still open below keep this ADR's stated default until the founder answers.

Not legal advice. Item 6 of the open questions lists what needs a lawyer. Repo facts are `path:line` on main at ec5ecff unless a branch is named. Nothing was built or run for this ADR: only git, grep, `cargo tree --offline` (dependency metadata, no build), crates.io API reads and the web pages under Sources, on 2026-10-21, on the founder's PC, with no model.

## Context

**The project's own licence is stated three ways.** `Cargo.toml:9` says `license = "MPL-2.0"`, and every crate inherits it. `README.md:109` says "None yet: all rights reserved". `docs/third-party-notices.md:3`, merged with PR #17, says "Kernel AI is MPL-2.0." No LICENSE, CONTRIBUTING, SECURITY or MAINTAINERS file exists, and no source file has an SPDX header (497 tracked files, 434 of them `.rs`). The repository has been public since 2026-10-20 22:18 UTC. The README said "MPL-2.0 (see docs/decisions.md)" from 052c6a8 (2026-10-15) until PR #11 (d4c9b76) merged at 2026-10-21 21:41 UTC, so a public MPL-2.0 statement stood for about 23 hours. Whatever we choose must be at least as generous as MPL-2.0 for that snapshot. Whether that line was a grant is for counsel.

**Who wrote the code.** At ec5ecff, `git rev-list --count origin/main` gives 207 commits. `git log --format=%ae | sort | uniq -c` gives 205 on the founder's noreply address, 1 on a second founder address, and 1 authored "Claude" (f27ba27, research memos only). `git log --format='%(trailers:key=Co-Authored-By,valueonly,separator=%x2C)' | grep -ci claude` gives 134 commits with an AI co-author trailer. None carries `Signed-off-by`. There has been no outside contribution, so the trigger "before the first outside contribution" (`docs/decisions.md:199`) has not fired. These numbers grow with every merge; quote the commands, not the numbers.

**The image owes notices and carries none.** `make_esp` (`xtask/src/esp.rs:54-84`) writes Limine's `BOOTX64.EFI` (`:70`), the kernel, the ring-3 programs and the components, and no licence file.
- Limine 12.9.3 is pinned by SHA-256 (`xtask/src/main.rs:61-62`, fetched by `ensure_limine` at `:325`). Its tarball has a 1,297-byte `LICENSE` (BSD-2-Clause) and an 89,220-byte, 1,715-line `3RDPARTY.md`, which "must accompany those binaries" (`3RDPARTY.md:9-13`). `BOOTX64.EFI` holds code under Apache-2.0 (PCG, `3RDPARTY.md:912-916`), Apache-2.0 WITH LLVM-exception, 0BSD, BSD-2-Clause, BSD-2-Clause-Patent, BSD-3-Clause, MIT and CC0-1.0 (BLAKE2, `:79-80`). The summary at `:20-92` also lists BSD-Source-Code-no-disclaimer for PicoEFI (`:47`), a BSD variant in no class below; whether that part reaches `BOOTX64.EFI` is unverified.
- `cargo tree -e normal,no-proc-macro` over `kernel` and `user` (target `x86_64-unknown-none`) and `comp-notes` (`wasm32-unknown-unknown`) finds 55 shipped crate versions at 32f2e74, and again at ec5ecff (rerun offline for this check). Cargo.lock is unchanged between them (`git diff --stat`).

| Licence of shipped crates | Count | Examples |
|---|---|---|
| MIT and Apache-2.0 (either), incl. 7 slash-form strings | 38 | bitflags 1.3.2, x86_64, wasmi |
| MIT only | 8 | libm, talc, virtio-drivers, noto-sans-mono-bitmap |
| Unlicense OR MIT | 2 | byteorder, memchr |
| 0BSD | 2 | smoltcp, managed |
| Zlib; BSD-2 OR Apache OR MIT; Apache WITH LLVM-exception OR Apache OR MIT | 1 each | foldhash; zerocopy; wasmparser 0.228 |
| MPL-2.0+ | 1 | smartstring 1.0.1, via rhai 1.26.1 (Cargo.lock:696, :856) |
| BlueOak-1.0.0 | 1 | minicbor 2.3.0, in every component (`compsdk/Cargo.toml:13`) |

- Outside Cargo.lock: `core`, `alloc` and `compiler_builtins` (the last is `MIT AND Apache-2.0 WITH LLVM-exception AND (MIT OR Apache-2.0)`). The console glyphs in `noto-sans-mono-bitmap` are Noto Sans Mono under OFL-1.1, "Copyright 2022 The Noto Project Authors", although the crate's metadata says MIT only. The 250-range table in `bpe/src/marks.rs:1-6` is derived from Unicode data (Unicode-3.0).
- rhai 1.27.0 was published 2026-10-21 13:05 UTC (MIT OR Apache-2.0). It still lists `smartstring ^1.0.0` as a required, non-optional dependency; `compact_str` is optional only (crates.io API, read 2026-10-21). Upgrading Rhai does not remove the MPL dependency. RUSTSEC-2026-0249 (dated 2026-05-03, added to the database 2026-08-10) marks smartstring unmaintained.

**Models.** `xtask/models.json:33,91,149` pins three Apache-2.0 models. `xtask/src/models/pin.rs:123-125` refuses any other licence than Apache-2.0 or MIT (`docs/decisions.md:84`). `:model` shows the GGUF's licence in the OS (`user/src/bin/llm/local.rs:266`). No weights are redistributed: `xtask setup` downloads them. The unsloth GGUF repo pinned for Qwen3-4B-Instruct-2507 holds no LICENSE file; the base repo's LICENSE is Apache-2.0, "Copyright 2024 Alibaba Cloud".

**Versions.** `kernel/Cargo.toml:3` is `0.10.0`, and `kernel/src/version.rs:5-6` says "0.10 is M10". 0.10.0 sorts above the planned v0.1. No tag, release workflow or published binary exists. Builds are reproducible on purpose (`.cargo/config.toml:19`, `xtask/src/image/mod.rs:16-20`, `rust-toolchain.toml:3`).

**The name.** `git grep -n 'Kernel AI'` gives 112 lines in 53 files, 66 of them in 40 `.rs` files. These include the model's system prompts (`llmproto/src/chat.rs:13`, `script.rs:138`, `explain.rs:14`, `tune.rs:259`), the banner (`version.rs:20`), the boot entry (`esp.rs:45`) and the VM name (`xtask/src/vbox/manage.rs:9`). HI LLC's KERNEL registration (reported in a search snippet; the record itself is unverified) and kernel.sh (hosted browsers for AI agents) are close marks.

**Rows this ADR touches.** `docs/decisions.md:12` ("no code is reused"), `:30` (engine clean-room), `:32` (no other new crates), `:52` ("Borrow ideas only, clean-room"), `:53` (MPL-2.0 as the working choice), `:84` (model licences), `:85` and `:124` (smartstring and minicbor, open), `:199` (MPL-2.0 with a CLA as the leading option) and `:207` (no licence). The founder's 2026-10-20 relaxation of reuse is not yet recorded. The founder's 2026-10-21 rule is: MIT, Apache-2.0, BSD, ISC or zlib crates may be added without asking if each PR lists them and their licences; GPL, LGPL and now MPL-2.0 need the founder's OK.

**Roadmap.** P0 lists release, licence and contribution terms (`docs/roadmap.md:139`), P8 lists licence and contributor terms and a release checklist (`:147`), and v0.1 is P1-P4 in a VM (`:153`).

## Options considered

Three designs were judged. Scores are the judge's, out of 10.

| Design | Score | Evidence for | Why it lost or won |
|---|---|---|---|
| A: MPL-2.0 with a CLA (Harmony 1.0 licence form, outbound "Option Five") | 6 | Keeps every business option open; it was the leading option at `decisions.md:199`. Lowest relicensing cost. | Lost. Needs counsel-drafted CLA texts (weeks) before any outside PR, plus a signatures file and a check to maintain. CLAs deter contributors: GitLab dropped its CLA in 2017 calling it "a deterrent", and seL4 dropped one in 2020. A CLA-backed relicence repeats HashiCorp's 2023 and Redis's 2024 trust problem. Its rule "CC0 never in shipped code" is already false: Limine ships CC0 BLAKE2. Its "MPL-2.0 to GPL costs nothing" overstates MPL s3.3, which allows GPL-family terms only for a Larger Work combined with such code, not a relicence. Its "Apache ICLA v2.2" is unverified. |
| B: Apache-2.0 OR MIT with the DCO | 7 | Maximum adoption and crates.io reuse (Rust API guidelines C-PERMISSIVE). Cheapest to change later: moving to MPL is free, leaving MPL is not. Least exposed to the AI-authorship question. | Lost. It reverses the settled working choice (`:53`), the public README history and PR #17's text. It gives up file-level copyleft on the agent's tier table and capability checks, with no way back for past releases. It puts the closed-fork decision on the founder without asking. With no per-file headers, a copied file loses its notice. Its claim that Tock requires a human sign-off is wrong: Tock requires no DCO and forbids AI-written PR descriptions. |
| C: MPL-2.0 with the DCO, no CLA | 8 | Matches `Cargo.toml:9`, PR #17's text and the 23-hour public statement. Permissive `kabi` and `compsdk` while the founder is their only author. MPL reuse (Asterinas, Servo, smartstring) needs no exception for our own licence. Lowest contributor friction (`git commit -s`). Its release dry-run spike is the strongest. | **Won**, with grafts. Gaps fixed below: no `kai.notices` (a `.wasm` shared alone carried minicbor's BlueOak link nowhere), no 0BSD examples, the name footprint understated, and the claim that smartstring "needs nothing" (it owes its notice and MPL s3.2 source availability). |

Grafts: from A, the 0BSD worked examples, `kai.notices`, model `.LICENSE` sidecars, `third_party/<name>/UPSTREAM.toml` and the dep-info scan. From B, `SECURITY.md`, the Cargo.lock hash in the notices, the model revision in release notes and the `PRODUCT_NAME` constant.

**Open-source reuse options weighed.**

| Option | Version, licence, date | Verdict |
|---|---|---|
| cargo-deny | 0.20.2, MIT OR Apache-2.0, 2026-07-09 | Adopted: licence allowlist and advisories in CI. Saves an SPDX checker and RustSec lookup (estimate 1-2 weeks). Host only. |
| cargo-about | 0.9.2, MIT OR Apache-2.0, 2026-08-18 | Adopted: licence-file discovery, target filtering, rendering. Saves about a week of xtask code (estimate). Host only. Misses OFL inside noto-sans-mono-bitmap and Limine's 3RDPARTY, hence the hand list. |
| REUSE spec | 3.3, 2024-11-14 | Adopted: header form and `REUSE.toml`. |
| `reuse` tool | 6.2.0, GPL-3.0-or-later code, 2025-10-27 (PyPI) | Not required. Our own `cargo xtask licences check` does the job; the tool stays optional and host-only. |
| DCO | 1.1, verbatim copying only | Adopted, linked from CONTRIBUTING.md. |
| DCO GitHub App | ISC (github.com/dcoapp/app) | Fallback only. A ~30-line CI job needs no third party. |
| CLA Assistant; CLA Assistant Lite | Apache-2.0; archived (last push 2026-03-23, archive date unverified) | Not adopted: no CLA now. |
| Harmony CLA 1.0, Option Five | templates, site CC-BY-3.0 | Held in reserve if question 1 is answered yes. |
| cargo-cyclonedx | 0.5.9, Apache-2.0, 2026-03-19 | P8, for an SBOM. Not needed for v0.1. |
| ASF A/B/X categories; Zephyr's import record; Linux's AI rule | policy pages | Used as ideas, in our own words. |

## Decision

**The founder's call, 2026-10-22:** decide the OS licence later, before v0.1. Until then the code stays "all rights reserved" and outside contributions are not accepted. D1 is the proposal for that later decision. The rest is accepted: D2's DCO sign-off and D3 apply once contributions open, and D4 onward hold now.

The proposal: Kernel AI stays under MPL-2.0, the licence its workspace manifest already names. Anyone who changes our files and ships the result must publish those changed files, but apps built on top may use any licence they like. Outside contributors add a one-line sign-off to each commit (`git commit -s`) instead of signing a contract with us. Every image and release carries a complete, CI-checked list of the third-party code inside it and where to get our source.

**D1. Outbound licence (deferred: decided before v0.1; all rights reserved until then).**
- Every file we write is `MPL-2.0`, with no Exhibit B, so MPL s3.3's secondary-licence route stays open. A root `LICENSE` holds the MPL-2.0 text. Docs are MPL-2.0 too. `README.md:107-109` ("all rights reserved") stays until the founder decides; it is replaced then.
- `kabi`, `compsdk` and future WIT files (ADR [0003](0003-app-model.md)) become `MIT OR Apache-2.0`, because third-party programs compile them in. This is free today: every line in them is the founder's.
- Worked examples that the model copies into users' scripts are `0BSD`, so AI output carries no notice duty. Today that is `EXAMPLES` at `llmproto/src/script.rs:97`, moved to `llmproto/src/examples.rs`. From P1 it is the skill pack's examples (ADR [0010](0010-skill-pack.md)).
- What the AI writes for the user belongs to the user. The OS claims nothing and shows no licence screen.

**D2. Inbound terms: DCO 1.1, inbound equals outbound, no CLA (the DCO sign-off is accepted and applies once outside contributions open; inbound-equals-outbound waits for D1).**
- `CONTRIBUTING.md` links DCO 1.1. Every non-maintainer commit carries `Signed-off-by: Name <email>` matching the author, with a known identity, as Linux's submitting-patches page asks.
- A `dco` job in `.github/workflows/ci.yml` (`fetch-depth: 0`, about 30 lines of shell) checks each non-merge commit in `base..head`. An author listed in `MAINTAINERS` passes. Any other author needs a sign-off with the same email.
- `MAINTAINERS` lists the founder's two commit identities, which are exempt. Any later maintainer signs off.
- The patent licence comes from MPL s2.1(b), with termination for whoever sues in s5.2. History is not rewritten. CONTRIBUTING.md records that everything before the adoption commit is the founder's, under MPL-2.0.
- If the founder names a dual-licence or relicensing plan (question 1), A's Harmony Option Five CLA is added for future contributions only. GitHub ToS D.6 (effective 2026-04-27) lets a separate agreement supersede its inbound=outbound rule. That rule applies only to repos "containing notice of a license", so it gives nothing until `LICENSE` lands.
- `cargo xtask provenance` reports, per crate, the share of lines (git blame) by non-maintainer identities. It is 0% today and must exist before the first outside PR is merged.

**D3. AI-assisted work.** Every commit has a human author. AI tools never add `Signed-off-by`, following Linux's rule ("AI agents MUST NOT add Signed-off-by tags", docs 7.3.0). The human records AI help with `Co-Authored-By: <model>` (the sessions' default today) or `Assisted-by: <model>`, and answers for the code. Neither trailer counts as a sign-off. Copyright in AI-assisted code is open (US Copyright Office Part 2, 2025-01-29); a CLA would not change that, because it can grant only what exists.

**D4. Reuse classes (decided 2026-10-22).** This applies the founder's 2026-10-21 rule and their 2026-10-22 answers.

| Class | SPDX ids | Rule |
|---|---|---|
| free | MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib; and, decided 2026-10-22, 0BSD, MIT-0, Unlicense, BSL-1.0, Unicode-3.0, CC0-1.0, BlueOak-1.0.0, BSD-2-Clause-Patent | Add without asking. The PR lists each new crate with its licence. Notices are kept and generated (D6). CC0-1.0 grants no patent licence; noted, accepted. |
| approved items | `smartstring` (MPL-2.0+, required by Rhai); OFL-1.1 font data: Noto Sans Mono and the fonts ADR [0005](0005-graphics.md) names (decided 2026-10-22: "keep smartstring (MPL-2.0) and the OFL fonts"; read here as OFL approved for fonts) | Kept with their notices. An OFL font keeps its copyright line and text, is never sold alone and drops any Reserved Font Name once converted. |
| ask | MPL-2.0 other than `smartstring` (file by file, usually approvable; one row per item); OFL-1.1 for anything but font data; GPL-\*, LGPL-\*, AGPL-\*; firmware blobs; everything else (EPL, CDDL, EUPL, SSPL, BUSL-1.1, CC-BY-SA, non-commercial, custom, none) | Not adopted until a founder-signed `docs/decisions.md` row names the item. An approved MPL file stays whole and MPL; an OFL font keeps its copyright line and text, is never sold alone and drops any Reserved Font Name once converted. Linux, glibc and GPL drivers stay ideas only. |

- MPL-2.0 being "ask" for dependencies is a separate matter from our own licence (D1).
- **C code in the build is allowed** (decided 2026-10-22), under the same classes: uACPI ([ADR 0007](0007-drivers.md)), xcc ([ADR 0012](0012-terminal-compiler.md)) and musl in ADR 0006's `kai-guest`.
- **GPL and other ask-class tools may run in CI, never shipped** (decided 2026-10-22): QEMU, e2fsprogs and blklogwrites (ADR 0004), `mke2fs` (ADR 0006), virt-firmware (ADR 0008). Nothing they produce links GPL code into the OS.
- **Still open (question 4):** copied source files under free licences, such as the RedoxFS allocator, FreeBSD's AMD-Vi and igc files, and Tock. Until the founder answers, each import asks, with one row per import.
- Models are a separate rule and unchanged: Apache-2.0 or MIT only (`:84`, `pin.rs:123`).
- **Import record.** A copied file keeps its upstream licence, including our later edits, so fixes can go back upstream. Its header carries the upstream SPDX id, the upstream copyright, our copyright line and `Upstream: <url> @ <commit> <path>`. The commit adds a trailer `Imported-from: <url>@<commit> (<SPDX id>)` and says why. Whole trees go to `third_party/<name>/` with an `UPSTREAM.toml` (name, url, commit, spdx, copyright, files, changes, why).

**D5. Headers and licence texts (REUSE 3.3).**
- Every tracked text file starts with `// SPDX-License-Identifier: MPL-2.0` and `// SPDX-FileCopyrightText: The Kernel AI contributors` (comment syntax per file type).
- Exceptions: `kabi` and `compsdk` use `MIT OR Apache-2.0`; the examples file uses `0BSD`; `bpe/src/marks.rs` uses `MPL-2.0 AND Unicode-3.0`; imports use their upstream ids.
- `REUSE.toml` covers files that cannot hold a comment: `xtask/goldens/*.json`, `.bin`, `.bf16` and `.png` fixtures, and `Cargo.lock`.
- `LICENSES/` holds one text per SPDX id used in the tree; MPL-2.0 is the root `LICENSE`.
- `cargo xtask licences check` (our code, about 150 lines, no new crate) fails CI on a tracked file with no header and no `REUSE.toml` match, on an id with no text in `LICENSES/`, or on an id outside the free class with no approving row.

**D6. Dependency gate and notices.**
- **`deny.toml`, run with cargo-deny 0.20.2 in `lint-and-unit` (`ci.yml:20`).** `allow` is the free class, plus the proposed free ids once approved. `exceptions` names each approved ask-class crate: smartstring (MPL-2.0+), approved 2026-10-22. `clarify` (pinned by file hash) sets noto-sans-mono-bitmap to `MIT AND OFL-1.1` and fixes slash-form strings if needed. `private.ignore = true`. Graph targets are `x86_64-unknown-none` and `wasm32-unknown-unknown`. `[advisories]` ignores RUSTSEC-2026-0249 with a reason that names a review date of 2027-01-31. ADR [0012](0012-terminal-compiler.md) owns the Rhai 1.27 upgrade and Rhai's string type; 1.27.0's `compact_str` feature changes the type but still requires smartstring.
- **`cargo xtask notices [--check]`** merges these inputs:
  1. cargo-about 0.9.2 output for the two shipped targets, with no build or dev dependencies;
  2. a hand list, `xtask/notices.json` (a build input beside `xtask/models.json`, not a runtime manifest): `core`, `alloc`, `compiler_builtins` from the pinned rust-src; Limine's `LICENSE` and `3RDPARTY.md` verbatim from the hash-pinned tarball; Noto Sans Mono's copyright line and OFL-1.1; Unicode-3.0 for `bpe/src/marks.rs`;
  3. the headers of non-MPL files in shipped crates.
  The file names no hash of `Cargo.lock` (dropped 2026-10-26): every PR that added a crate changed that line and conflicted with every other, and `--check` regenerates the content, so a stale file is caught without it.
- **The output.** A committed root `NOTICES.txt`, plain UTF-8: one section per shipped part, then each licence text once with its copyright lines. CI fails if it is stale, so only CI and whoever changes dependencies need cargo-about.
- **On the image.** `make_esp` copies `NOTICES.txt` to the ESP root with one line prepended: `Kernel AI <version> (commit <sha>). Licence MPL-2.0. Source: https://github.com/dane1599d/kernel-ai/tree/<tag or commit>`. That meets the binary-notice duties of MIT, BSD, OFL and BlueOak and Limine's 3RDPARTY rule. Through the source URL it also meets MPL s3.2's duty to tell recipients where the source is. When ADR [0008](0008-updates.md)'s slots land, each slot image carries its own copy and the ESP keeps the one for Limine.
- **Components.** `pack` (`xtask/src/components.rs:154-166`) appends a third custom section, `kai.notices`: the component's own SPDX expression, then the notices of its linked crates (today minicbor's BlueOak link plus MIT texts, a few KiB against `MAX_ARTIFACT` of 1 MiB, `kabi/src/live.rs:186`). Hosts look sections up by name (`livecore/src/artifact.rs:43`), so existing hosts ignore it. ADR [0003](0003-app-model.md) owns the section list; this ADR owns its content. `kai-manifest 2` (0003) and `kai-package 1` (ADR [0011](0011-packages.md)) carry an SPDX licence expression whose ids must pass D4.
- **Models.** `xtask setup` writes `<file>.LICENSE` (the base repo's Apache-2.0 text) and `<file>.provenance.toml` (Hub repo, revision, base model, SHA-256) beside the GGUF. In the OS from P5, the licence comes from the model's target in the `models` role of 0008's one TUF repository, and the check in `pin.rs:120-143` moves into a no_std table shared with xtask.

**D7. Versions, tags, channels and releases.**
- **One version.** `[workspace.package] version = "0.1.0-dev"`. Every crate uses `version.workspace = true`, `kernel/Cargo.toml:3` included. The "0.10 is M10" rule (`version.rs:5-6`) goes; `MILESTONE` stays a banner field: "Kernel AI 0.1.0-dev (M10a, commit …)".
- **SemVer 2.0.0 for humans.** Tags are `vX.Y.Z-rc.N` and `vX.Y.Z`, and only go up. Updates are ordered by 0008's `release_seq`, never by SemVer. Syscall ABI (`kabi::ABI`, ADR [0001](0001-capabilities.md)) and format versions are separate numbers.
- **Channels.** `dev` (CI images from main, artefacts kept 30 days), `rc` (`-rc.N` tags) and `stable` (plain tags). 0008 signs `rc` and `stable` as TUF targets; `dev` builds are never signed by the real root. Changing channel never wipes user data.
- **What a release holds.** `.github/workflows/release.yml` runs on a `v*` tag and drafts a GitHub Release on `dane1599d/kernel-ai`; the founder publishes it by hand. 0008 owns the build-twice, signing and TUF steps in the same workflow, and publishes the same bytes as targets in `dane1599d/kernel-ai-updates`. The assets:
  1. `kernel-ai-<v>-x86_64.img.zst`, the GPT image, which is also P7's USB stick;
  2. `NOTICES.txt`, byte-identical to the image's copy;
  3. `kernel-ai-<v>-source.tar.zst`: `git archive` of the tag, plus `cargo vendor --locked`, plus Limine's pinned tarball. This gives smartstring's MPL source with every release. `ensure_limine` (`xtask/src/main.rs:325`), which today always fetches with curl, learns to use an archive already present after the hash check, so the tarball rebuilds offline;
  4. `SHA256SUMS`, signed as 0008 sets (it is a TUF target);
  5. notes naming the unbundled model: unsloth/Qwen3-4B-Instruct-2507-GGUF at the revision and SHA-256 in `xtask/models.json`, base Qwen/Qwen3-4B-Instruct-2507, Apache-2.0.
- **No EULA and no click-through.** MPL-2.0 needs no acceptance.
- **`SECURITY.md`** points to GitHub's private vulnerability reporting. EU CRA reporting duties apply from 2026-09-11, per a law-firm alert (EUR-Lex not read).

**D8. The name.**
- `TRADEMARKS.md` says the code licences grant no rights in "Kernel AI" (MPL s2.3). An unmodified tag may be called Kernel AI. A modified build renames, or says "based on Kernel AI" and changes the banner and boot entry.
- One `PRODUCT_NAME` in `kabi`, which kernel, user, shellcore and xtask already depend on; llmproto gains the dependency. It is a `macro_rules!` name as well as a constant, so `concat!` keeps the prompts `const`. The banner, the boot entry, the VM name and the four prompts use it. A test checks that the prompt bytes do not change, so this refactor needs no fluency re-run; a later rename does.
- The seeds `kernel-ai` (`image/mod.rs:20`, `.cargo/config.toml:19`) stay, because changing them changes every image hash.
- No filing before counsel's clearance search, which comes before the v0.1 tag.

**D9. Rows.** One new `docs/decisions.md` row, "Licence and reuse terms (ADR 0009)":
- supersedes `:30`, `:52`, the "no code is reused" clause of `:12`, and `:32` (replaced by D4 and the 2026-10-21 rule);
- settles `:53` and `:199` (MPL-2.0 with the DCO, no CLA unless question 1 says otherwise);
- closes `:85`, `:124` and `:207` once questions 2 and 3 are answered;
- keeps `:84`.

## Consequences

- The first public image carries complete, CI-checked notices and a source path. Today it carries none. Nothing is retrofitted at P8.
- Anyone who ships a changed Kernel AI must publish the changed files, including the agent's tier table and capability checks. Apps and whole products may still be proprietary (MPL FAQ Q11), and app authors meet only permissive ABI crates.
- Contributors add one flag (`-s`). There is no CLA, signing service or signature file. Founder-authored session commits pass the `dco` job unchanged.
- Reuse opens up under clear rules: llama.cpp and ggml kernels (MIT) for the engine, Redox drivers (MIT), and Asterinas or Servo files (MPL-2.0) with a founder row.
- **Costs.**
  - The founder gives up the unilateral right to take outside contributions closed, source-available or dual-licensed. A CLA later covers future contributions only.
  - Every distributed image, CI artefacts and updates included, owes MPL source availability. The tag plus the source tarball carries that.
  - `NOTICES.txt` is generated and committed, so it changes with each dependency change, and CI needs cargo-about.
  - The header pass covers every tracked file once (497 at ec5ecff), by header or `REUSE.toml`.
  - MPL is ASF Category B and is kept off Fuchsia production targets, so only the ABI crates reach those projects.
  - The maintainer exemption makes the provenance record uneven.
  - The DCO records a promise and verifies nothing; a CLA verifies nothing either.
  - Investors may prefer a CLA-held codebase (common belief; unverified).
- **Effort.** About 9-12 engineering days (estimate), the sum of the nine PRs listed under "Changes to recently merged and open work", plus counsel time for the name.

## What it costs to change later

| Change | Cost |
|---|---|
| Relicense files only maintainers wrote | Free at any time (`cargo xtask provenance` lists them). |
| Our files in a GPL-family combined work; a later MPL version | Free: MPL s3.3 and s10.2. Not a relicence of the files themselves. |
| Add a CLA | Covers future contributions only (ToS D.6 lets it supersede). Past contributions stay MPL-only. |
| Move outside-authored files to terms outside s3.3 and s10.2 | Each author's consent or a rewrite. OpenStreetMap's move took years, ended on 2012-09-12 and lost "some 1% of data". Grows with every outside line; provenance shows the bill. |
| Make `kabi` or `compsdk` copyleft again | Not possible for released versions. |
| Headers, notices tooling, channel names | Internal and mechanical; the data (headers, `notices.json`) outlives the tools. |
| Version scheme | Free until the first tag. After it, 0.1.0 must follow every published number. |
| The name | One constant now, plus a fluency re-run. After v0.1: users, links and any filing. |

## Spikes that settle it

**S09-1, release dry run.** GitHub Actions only: `ubuntu-24.04` runners plus one `windows-2025` job. No model, no LLM, never on the founder's PC. It runs with 0008's S08-2 (slot size and reproducibility).

Build on a throwaway branch: `deny.toml`, cargo-about config and template, `xtask/notices.json`, `cargo xtask notices`, the ESP copy, `release.yml` in draft-only mode, the source tarball and the offline Limine path.

| # | Measure | Pass | Fail and what follows |
|---|---|---|---|
| 1 | Coverage. Every crate version in `cargo tree -e normal,no-proc-macro` for the three shipped graphs (55 at 32f2e74; recount at the spike commit), plus `core`, `alloc`, `compiler_builtins` and every hand item, has an entry with licence text and copyright lines. Also scan the rustc dep-info (`.d`) source lists of the kernel, ring-3 and component builds for `Copyright\|SPDX-License-Identifier\|licen[cs]e\|OFL\|GPL\|Unicode`. | Zero unknown or NOASSERTION entries; zero scan hits not covered. `cargo deny check licenses` passes with only the expected exception (smartstring) and clarify (noto). | Any gap: fix it, and the dep-info scan becomes a permanent CI check. |
| 2 | Limine | Its two sections in `NOTICES.txt` are byte-identical to the pinned tarball's files. | Fix before any tag. |
| 3 | Determinism | `NOTICES.txt` is identical on Linux and Windows runners. `cargo xtask image` gives the same SHA-256 on two fresh Linux runners. | Fix before any tag. |
| 4 | Offline rebuild | From the source tarball in `docker run --network none`: no fetch, same image SHA-256. | Fix before any tag. |
| 5 | Custom target | Graphs for `user/x86_64-kai-user.json` match the `x86_64-unknown-none` stand-in. | xtask walks `cargo metadata` itself (about 300 lines). |
| 6 | Cost | `NOTICES.txt` at most 512 KiB (fail above 1 MiB). At most 90 s added to `lint-and-unit` with cached tools. Whole dry run under 30 min. KVM `image` boot-to-ready within 2% of main. | Over 90 s: notices run on main and tags only. Over 1 MiB: deduplicate texts harder. |

Also recorded, with no threshold: the source tarball's size (unverified) and whether cargo-deny accepts the seven slash-form strings (unverified).

The `dco` and `licences check` jobs are tested in their own PRs with a fixture branch: an unsigned non-maintainer commit must fail, and a signed one must pass.

## Phases that depend on it

| Phase | Needs |
|---|---|
| P1 | `system_about` (read-only, run tier) in ADR [0002](0002-agent-actions.md)'s registry: version, commit, licence, source URL and a notices summary from `NOTICES.txt`. 0BSD examples and skill pack text (0010). The `dco`, `deny` and `licences check` gates before any outside PR. |
| P2 | `NOTICES.txt` and the model sidecars find their place on disk (ADR [0004](0004-storage.md)); each slot carries its own `NOTICES.txt` (0008). |
| P3 | Each font is an ask-class OFL entry unless it is free class (Spleen BSD-2-Clause, Cozette MIT). A Settings About page reads what `system_about` reads (ADR [0005](0005-graphics.md)). |
| P4 | `kai.notices` in every app artifact; an SPDX licence in `kai-manifest 2` and `kai-package 1`, checked by D4 (0003, 0011). The model manager shows the licence. In-OS C builds (0012) wait on question 4. |
| P5 | rustls 0.23.46 (Apache-2.0 OR ISC OR MIT) is free class; the RustCrypto provider crates (`docs/roadmap.md:144`) are listed in their PR with licences (not checked here). ring and aws-lc-sys build C code, so they wait on question 4. The in-OS model licence check. US export classification (counsel). |
| P6 | The Linux guest image (GPL-2.0 kernel, GPL and LGPL user land), Firefox (MPL-2.0) and codecs are ask class (ADR [0006](0006-existing-software.md)). |
| P7 | The USB stick carries `NOTICES.txt` already; firmware blobs are ask class (ADR [0007](0007-drivers.md)). |
| P8 | An SBOM from cargo-cyclonedx 0.5.9, the CRA review, the trademark filing, and a CLA if question 1 chooses one. |

v0.1 needs D1-D8 in place before its tag.

## Changes to recently merged and open work

| Work | What a follow-up PR changes, and why |
|---|---|
| PR #17 terminal look (merged) | Its "Kernel AI is MPL-2.0." stays true. The notices PR moves its Noto entry (copyright line, OFL-1.1, the crate's MIT text) into `xtask/notices.json` and `LICENSES/OFL-1.1.txt`, and deletes `docs/third-party-notices.md` in favour of the generated `NOTICES.txt`, because a repo-only doc never reaches the drive. The header PR adds two lines to `kernel/src/console/{mod,fb,layout,status,theme}.rs` and `termcore/src/style.rs`. The wider Noto features (bold; 16, 20, 24, 32 px) stay one OFL entry. |
| PR #15 seamless actions (merged) | `EXAMPLES` moves from `llmproto/src/script.rs:97` to `llmproto/src/examples.rs` under 0BSD now; the behaviour is unchanged. `shellcore/src/tiers.rs` and `undo.rs`, `show.rs`, `log.rs` get headers. P1 adds `system_about` as a run-tier row in 0002's registry (`agentabi`), which generalises `TIERS` (`tiers.rs:35`). |
| PR #14 AVX2 attention (merged) | Nothing now. Superseding `:30` lets later work port ggml or llama.cpp kernels (MIT) with the D4 header and trailer. |
| PR #13 parser hardening; PR #10 fluency (merged) | Headers only. The `PRODUCT_NAME` refactor keeps prompt bytes identical, so no fluency re-run; a rename would re-run `fluency-model.yml` on GitHub Actions against PR #10's 47/50 (run 39675848260). |
| PR #16 DHCP, DNS, ping (open, `origin/dane/net-dhcp-dns-b5bdb7`) | No change before merge. `netcore` inherits MPL-2.0 (`license.workspace = true`). smoltcp gains features, not crates; it stays 0BSD, in the proposed free class (question 2). `xtask/src/mock_dns.rs` is host-only. |
| PR #18 virtio-gpu 2D (open) | No change before merge; `gfxcore` inherits MPL-2.0. |
| PR #19 USB input (open) | No change before merge. `xhci/src/lib.rs:16-18`'s "no code was taken from another driver" note stays the record. Adopting Redox xhcid code (MIT) later needs MIT headers and an `Imported-from:` trailer. |
| All sessions | Founder-authored commits pass the `dco` job unchanged. The header pass lands after #16, #18 and #19 merge, or each branch takes it on rebase. |

Follow-up PRs, in order:
1. **licence** (half a day): `LICENSE`, `CONTRIBUTING.md`, `MAINTAINERS`, `SECURITY.md`, `TRADEMARKS.md`; `README.md:107-109`; `license = "MIT OR Apache-2.0"` in `kabi/Cargo.toml` and `compsdk/Cargo.toml`; the D9 row.
2. **ci** (1 day): the `dco` job and `deny.toml` with cargo-deny 0.20.2 in `.github/workflows/ci.yml`.
3. **examples** (hours): `llmproto/src/examples.rs` (0BSD).
4. **version** (hours): root `Cargo.toml`, every crate's `version.workspace = true`, `kernel/src/version.rs:5-6`.
5. **headers** (1-2 days, scripted): all tracked files, `REUSE.toml`, `LICENSES/`, `cargo xtask licences check`.
6. **notices** (2-3 days, after S09-1): `xtask/notices.json`, cargo-about config, `cargo xtask notices`, `xtask/src/esp.rs`, `kai.notices` in `xtask/src/components.rs`, sidecars in `xtask/src/models/setup.rs`, `NOTICES.txt`; delete `docs/third-party-notices.md`.
7. **release** (2-3 days): `.github/workflows/release.yml` (with 0008), the offline path in `ensure_limine`.
8. **name** (1 day): `PRODUCT_NAME` in `kabi`, used at `version.rs:20`, `esp.rs:45`, `vbox/manage.rs:9` and the four prompts.
9. **provenance** (1 day, before the first outside PR): `cargo xtask provenance`.

## Open questions for the founder

1. **Business model. Deferred by the founder, 2026-10-22:** the OS licence is decided later, before v0.1. Until then the code stays "all rights reserved". The question stands for that decision: keep the option to relicense (a CLA), MPL-2.0 with the DCO (D1-D2), or Apache-2.0 OR MIT if a competitor's closed fork is acceptable.
2. **Decided 2026-10-22:** 0BSD, MIT-0, Unlicense, BSL-1.0, Unicode-3.0, CC0-1.0, BlueOak-1.0.0 and BSD-2-Clause-Patent join the free class.
3. **Decided 2026-10-22:** keep `smartstring` (MPL-2.0+) and the OFL fonts. Other MPL-2.0 items stay "ask", one row per item (for example `psl` in ADR 0001, pubgrub in ADR 0011).
4. **Partly decided 2026-10-22:** C code compiled into the image is allowed, and GPL tools may run in CI but never ship. Still open: do copied source files under free licences (the RedoxFS allocator, FreeBSD's AMD-Vi and igc, OpenBSD's rge, Tock, libabr) count as free, or does each import need a row?
5. **Deferred by the founder, 2026-10-22:** no outside contributions before v0.1. Whether to accept AI-assisted contributions under a human sign-off is asked again when contributions open.
6. **The name and counsel.** Keep "Kernel AI" pending a clearance search, or rename before v0.1? What budget for counsel: name, AI authorship, MPL source duty for images and updates, the 23-hour MPL README, CRA, export?
7. **Ownership.** Should a company hold your copyright and the mark, now or before v1.0? Contributors keep theirs.
8. **GPL and LGPL in P6.** A standing policy for the Linux guest, or case by case (0006's question 1)?
9. **Weights.** Will an image ever bundle model weights? That brings Apache-2.0 s4 notices for unsloth's quantisation and EU AI Act questions.
10. **AI trailer.** Keep `Co-Authored-By: Claude` or switch sessions to `Assisted-by:`? This is your session setting.

## Sources

External, read 2026-10-21 unless dated:
- MPL-2.0 and FAQ (updated 2024-01-30): https://www.mozilla.org/en-US/MPL/2.0/, https://www.mozilla.org/en-US/MPL/2.0/FAQ/
- DCO 1.1: https://developercertificate.org/
- Linux docs 7.3.0: https://docs.kernel.org/process/coding-assistants.html, https://docs.kernel.org/process/submitting-patches.html
- GitHub Terms of Service D.6 (effective 2026-04-27): https://docs.github.com/en/site-policy/github-terms/github-terms-of-service
- Harmony Option Five: https://www.harmonyagreements.org/guide.html
- REUSE 3.3 (2024-11-14): https://reuse.software/spec-3.3/
- cargo-deny 0.20.2 (2026-07-09): https://crates.io/crates/cargo-deny, https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html
- cargo-about 0.9.2 (2026-08-18): https://crates.io/crates/cargo-about
- cargo-cyclonedx 0.5.9 (2026-03-19): https://crates.io/crates/cargo-cyclonedx
- rhai 1.27.0 (2026-10-21T13:05Z): https://crates.io/api/v1/crates/rhai/1.27.0/dependencies; rustls 0.23.46 (2026-10-14): https://crates.io/crates/rustls
- ASF licence categories: https://www.apache.org/legal/resolved.html
- Rust API guidelines C-PERMISSIVE: https://rust-lang.github.io/api-guidelines/necessities.html
- Zephyr contribution guidelines (DCO; AI agents must not sign off): https://docs.zephyrproject.org/latest/contribute/guidelines.html
- Tock CONTRIBUTING.md, "AI Policy" (no DCO; PR text not AI-written): https://github.com/tock/tock/blob/master/.github/CONTRIBUTING.md
- seL4 DCO page (no CLA): https://sel4.systems/Contribute/dco.html; Redox CONTRIBUTING.md, "AI Policy" (no LLM-generated contributions): https://gitlab.redox-os.org/redox-os/redox/-/blob/master/CONTRIBUTING.md
- Fuchsia open source licensing policies (MPL-2.0 for development targets only; page updated 2025-07-31): https://fuchsia.dev/docs/contribute/governance/policy/open-source-licensing-policies
- RUSTSEC-2026-0249, smartstring unmaintained (dated 2026-05-03, added to the database 2026-08-10): https://rustsec.org/advisories/RUSTSEC-2026-0249.html; `reuse` 6.2.0 on PyPI: https://pypi.org/project/reuse/6.2.0/
- GitLab's switch to the DCO (2017-11-01): https://about.gitlab.com/blog/gitlab-switches-to-dco-license/
- HashiCorp BSL (2023-08-10): https://www.hashicorp.com/en/blog/hashicorp-adopts-business-source-license; Redis FAQ (2024): https://redis.io/blog/redis-adopts-dual-source-available-licensing/
- OpenStreetMap ODbL move: https://wiki.openstreetmap.org/wiki/Open_Database_License
- US Copyright Office, AI Part 2 (2025-01-29): https://www.copyright.gov/ai/; Thaler v. Perlmutter, cert. denied 2026-03-02: https://www.supremecourt.gov/docket/docketfiles/html/public/25-449.html
- Limine 12.9.3 release tarball (`LICENSE`, `3RDPARTY.md`): https://github.com/Limine-Bootloader/Limine/releases/tag/v12.9.3
- OFL FAQ 1.1-update7 (November 2023; page updated 2026-01-12): https://openfontlicense.org/ofl-faq/; Noto `OFL.txt`: https://github.com/notofonts/latin-greek-cyrillic
- BlueOak-1.0.0: https://blueoakcouncil.org/license/1.0.0; Unicode licence: https://www.unicode.org/license.txt
- SPDX licence list 3.29.0 (2026-09-16): https://raw.githubusercontent.com/spdx/license-list-data/main/json/licenses.json
- Qwen3-4B-Instruct-2507 and the unsloth GGUF: https://huggingface.co/api/models/Qwen/Qwen3-4B-Instruct-2507, https://huggingface.co/api/models/unsloth/Qwen3-4B-Instruct-2507-GGUF
- DCO GitHub App (ISC): https://github.com/dcoapp/app; CLA Assistant Lite (archived): https://github.com/contributor-assistant/github-action
- EU CRA reporting from 2026-09-11 (Goodwin alert of that date, citing Article 71(2); EUR-Lex not read): https://www.goodwinlaw.com/en/insights/publications/2026/09/alerts-lifesciences-technology-preparing-for-eu-cyber-resilience-act

Repo (main at ec5ecff; branches as named): `Cargo.toml:9`, `README.md:107-109`, `docs/decisions.md:12,30,32,52,53,84,85,124,199,207`, `docs/roadmap.md:139,144,147,153`, `docs/third-party-notices.md:3`, `kernel/Cargo.toml:3,35`, `kernel/src/version.rs:5-6,20`, `xtask/src/esp.rs:45,54-84,70`, `xtask/src/main.rs:61-62,325`, `xtask/src/image/mod.rs:16,20`, `xtask/src/vbox/manage.rs:9`, `xtask/src/components.rs:154-166`, `xtask/src/models/pin.rs:120-143`, `xtask/src/models/setup.rs:24-27`, `components/notes/Cargo.toml:2`, `xtask/models.json:33,91,149`, `livecore/src/artifact.rs:1-3,43,102`, `kabi/src/live.rs:186`, `compsdk/Cargo.toml:13`, `bpe/src/marks.rs:1-6`, `llmproto/src/script.rs:97,138`, `llmproto/src/chat.rs:13`, `llmproto/src/explain.rs:14`, `llmproto/src/tune.rs:259`, `user/src/bin/llm/local.rs:266`, `shellcore/src/tiers.rs:35`, `.github/workflows/ci.yml:20`, `.cargo/config.toml:19`, `rust-toolchain.toml:3`, `Cargo.lock:573,613,696,856`; `xhci/src/lib.rs:16-18` on `origin/dane/usb-input-83eb7c`; `netcore` on `origin/dane/net-dhcp-dns-b5bdb7`. Inputs: `D:/kai-targets/p0/work/decision-sheet.md`, `D:/kai-targets/p0/work/09/` (judgement, designs A-C, research). Limine's files (`LICENSE`, `3RDPARTY.md:9-13,20-92,912-916`) as extracted at `D:/kai-targets/main/limine-12.9.3/limine-binary/` and rechecked for this ADR.
