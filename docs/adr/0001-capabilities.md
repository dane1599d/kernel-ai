# ADR 0001: The capability and permission model

Proposed, 2026-10-21. For the founder's approval; nothing here is decided until approved.

Repo lines are cited on main at ec5ecff unless a branch is named. Open PRs were read with `git show` on their branches: #16 at 131aa77, #18 at af7635d, #19 at 0717caa. Nothing was built or run for this ADR. It follows the topic 01 research, the three designs and the judge's decision (all 2026-10-21), and the P0 decision sheet, which wins where they differ.

## Context

What exists on main today:

- **A 9-bit mask fixed at spawn.** SUPERVISE, SERVE, NET, CALL, POLICY, COMPONENT, PUBLISH, MODEL and THREADS (kabi/src/lib.rs:81-109), carried in `Limits.caps` (kabi/src/lib.rs:180-189) and tested per call by `need()` (kernel/src/process/syscall.rs:49-55). Nothing lowers, passes on or revokes a bit. Killing the process is the only revocation.
- **Grant rules by name, on one path.** SPAWN checks names and programs (kernel/src/process/supervision.rs:265-283). Processes the kernel starts skip those rules (kernel/src/thread_tests.rs:229), and MODEL_MAP admits any of them that holds MODEL (kernel/src/model.rs:284).
- **Ambient calls.** CALL reaches any service by name. "Calls never chain" is a check of names and live holders (kernel/src/process/syscall.rs:337-358, kernel/src/process/mod.rs:462-468), so a CALL holder that is between restarts is not excluded. METRICS needs no bit, so any process lists every pid and name (kernel/src/process/syscall.rs:118-129).
- **The user is "caller 0".** The kernel terminal sends typed lines to `shell` (kernel/src/terminal.rs:174-181). It drops keys typed before a question (:107-111), refuses `answer` at the normal prompt (:137) and declines a question it cannot show in full (:211-217). Three services obey caller 0 alone (user/src/bin/shell.rs:143, editcore/src/service.rs:95, livecore/src/host.rs:367).
- **The agent's bound is a language sandbox.** LLM-written scripts run inside `shell`, which holds serve, call and policy (kabi/src/plan.rs:9), under the strict Rhai profile (decisions.md:74). Seamless actions (#15, merged) added run, ask and never tiers per function (shellcore/src/tiers.rs:35-44), checked in the shell, not the kernel.
- **No versions, no persistence.** 37 calls are numbered in claim order (kabi/src/call.rs:4-125; decisions.md:123). There are 12 error codes, and unknown codes decode as `Invalid` (kabi/src/lib.rs:25-78). Struct sizes are pinned (kabi/src/lib.rs:280-292). The ELF parser ignores PT_NOTE and other unknown segments (uelf/src/lib.rs:130-133). The plan is rebuilt at each boot (kernel/src/selftest.rs:129-200), and `kai-manifest 1` has exactly 7 keys (livecore/src/artifact.rs:102-124).

What the roadmap needs: "Capabilities cover processes, apps, files, the network and agent actions alike" (roadmap.md:129); versioned ABIs and formats (:131); a P1 exit test that "text the agent reads from data cannot raise an action's tier" (:140, risk row :187); file rights in P2 (:141); per-app network permission in P5 (:144). The founder wants plain requests to "just happen", and the compiler-in-terminal research memo (2026-10-14) notes that users approve 93% of agent permission prompts, so confirmation is not a boundary (docs/research/compiler-in-terminal.md:46). It also says "Handle checks in the kernel are ground truth" (:249). No decisions.md row decides object capabilities, revocation, persistence, manifests or ABI versions.

## Options considered

The judge weighed three designs.

| Design | Score | Evidence | Outcome |
|---|---|---|---|
| A: kernel object capabilities (Zircon-style handles, seL4-style revocation tree, powerbox) | 8 | Current capability kernels give each process a table of handles whose rights only shrink (seL4, Zircon, Genode), and Redox began moving its namespaces onto file-descriptor capabilities in March 2026. Per-request handles bound the agent in the kernel. Handles stay private behind user/src/sys.rs, so only the app ABI, the manifest words and the grant file lock in. Closes the three gaps above. | **Base.** Weak points: thin user-facing words, no provenance labels, app surface left to ADR 0003, new revocation code with no proof. |
| B: named permissions and a ring-3 policy service | 5 | Android, macOS and Flatpak words users know; a Settings page; audit lines. | **Lost.** Host and path strings matched in ring 0; no delegation; revoking kills the app; the taint bit lives in user space; GRANT_REQUEST relayed "as caller 0" dilutes the user's identity (COMP_CONTROL relays only three read-only verbs, decisions.md:111). Not smaller than A (about 6,200 lines for its steps 1-6, B's estimate). B itself says moving to handles after v1.0 means "a compatibility layer for years". **Kept:** its words, classes, lifetimes and audit wording, and its finding that RESOLVE widens NET. |
| C: WASI-native capabilities | 7 | Same kernel layer as A; app exports as actions; the request mints handles; provenance labels on values; the most reuse (wit-parser, wit-bindgen, Wasmtime as an oracle). | **Lost as base.** It ties the model to a hand-written canonical ABI on wasmi (its own top risk) and decides much of ADR 0003 here; 14-18 weeks by its estimate; scripts stay in the shell for v0.1. Two errors. First, wasmparser 0.262.0 is in Cargo.lock only through `wat`, a host-side dev-dependency of wasmhost and xtask (Cargo.lock:1199-1202); the OS ships 0.228.0 through wasmi 2.0.0 (Cargo.lock:1153-1156, :1190-1193). Second, returning a badge in place of the caller's pid breaks the caller-0 checks. **Kept:** `kai-app 1` routes (`use`, `expose`), "routes form no cycle", provenance labels (now ADR 0002's), WIT resources mapped to kinds (ADR 0003 builds it). |

Open-source reuse weighed (licences read from crates.io, LICENSE files or SPDX headers on 2026-10-21):

| Project | Licence | Use | Verdict |
|---|---|---|---|
| Zircon handles and rights (Fuchsia) | MIT (kernel) | Rights names, subset rule, replace, move on transfer, 0 invalid, private syscall numbers. C++, so a design port. | Adopted (no code copied) |
| Tock 2.3.0 `kernel/src/capabilities.rs` | Apache-2.0 OR MIT | `unsafe trait` mint authority, checked by the compiler at no run-time cost | Adopted (pattern) |
| Tock `capsules/extra/src/net/network_capabilities.rs` (173 lines on master) | Apache-2.0 OR MIT | Address and port ranges for `NetDest` in P5, header kept | Adopted (port) |
| `bitflags` 2.13.2 | MIT OR Apache-2.0 | `Rights`. Already in Cargo.lock (Cargo.lock:75-78; kernel: x86_64, virtio-drivers; ring 3: rhai, wasmparser); a new direct dependency of `kabi`, nothing new to download | Adopted, free class |
| `cap-std` 4.0.3 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Path-beneath resolution for `Dir` (the crate needs `std`, so ported) | Adopted via ADR 0004 |
| `psl` 2.1.244, `psl-types` 2.0.11 | MIT/Apache-2.0; the embedded Public Suffix List data is MPL-2.0 | Refuse `*.co.uk`-style host grants in P5 | Founder question (MPL) |
| `wit-parser` 0.262.0, `wit-bindgen` 0.63.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Build-time WIT tooling | Via ADR 0003, free class |
| `wasm_component_layer` 0.1.18 (2024-08-02) | Apache-2.0 | Component model on wasmi, which wasmi's maintainer pointed to in issue #1665 (2025-09-20; closed 2025-09-23) | Not adopted: stale; `no_std` unverified |
| seL4 16.0.0 derivation tree | GPLv2 (kernel) | "Delete all child capabilities" | Idea only |
| Genode session routing | AGPLv3 with a linking exception | Label-based routing; destroying a session revokes its capabilities | Idea only |
| Flatpak document portal | LGPL-2.1-or-later | Per-app file grants with a `persistent` flag | Idea only |
| Hubris leases | MPL-2.0 | Call-scoped grants | Idea only |
| CaMeL, Progent, FIDES, AgentDojo | Apache-2.0, MIT, MIT, MIT | Value labels; "narrowing runs, widening asks"; injection test patterns | Ideas, via ADR 0002 and spike S01-2 |
| regorus 0.13.0, cedar-policy 4.14.0, biscuit-auth 6.0.0 | MIT AND Apache-2.0 AND BSD-3-Clause; Apache-2.0; Apache-2.0 | Policy engines and signed tokens | Not adopted: a table suffices; cedar and biscuit need `std` (cedar unverified) |

## Decision

Every program, every app and the AI hold keys, called handles, one for each thing they may use, and the kernel checks the key on every call. A key can be narrowed or passed on, never widened, and it can be taken back at once without stopping the program. For each request you type, the AI gets keys for that request alone, taken back when the answer appears; an action with no undo waits for your `y`, which makes a one-use key for exactly what the question showed. Text the AI reads in files or web pages can never make a key.

In design terms: base design A, with C's app-facing routes and B's user-facing words. Not B's kernel string checks.

### D1. Handles and rights

- **Handle.** `kabi::handle::Handle(u32)`. 0 is never valid. Bits 0-11 are the table index and bits 12-31 a generation. A slot whose generation would wrap is retired, never reused.
- **Table.** One per process, allocated at spawn with 256 entries and grown on demand to at most 4,096. An entry is about 40 bytes: kind, flags, rights, object and its generation, grant node and its generation, a 64-bit badge. Tables live in `HANDLES[MAX_PROCESSES]`, outside `Process`, because `Process` is `Copy` and `identity()` copies it under the table lock (kernel/src/process/mod.rs:109-110, :416-418). Lock order: SPACE, then HANDLES, then TABLE (extends mod.rs:17-19).
- **Rights.** A `bitflags` `u32` with Zircon's names where one exists: DUPLICATE, TRANSFER, INSPECT, WAIT, READ, WRITE, MAP, ENUMERATE, CREATE, DELETE, CALL, RECV, MINT, REVOKE, SPAWN, CONNECT, RESOLVE, SET_POLICY, THREADS and USER. There is no EXECUTE right: no memory becomes executable at run time before v1.0 (sheet; [ADR 0012](0012-terminal-compiler.md)).
- **Rules (Zircon's).** HANDLE_DUP asks for a subset of the source's rights or fails `Invalid`. HANDLE_REPLACE swaps a handle for a narrower one. A handle sent in a message moves. HANDLE_CLOSE always works. The flag `ONCE` makes a handle consumed by its first use.
- **Errors.** `BadHandle` (forged, stale or wrong kind) and `Revoked` take the next free codes: 13 and 14 on main today (kabi/src/lib.rs:25-45). Old programs read them as `Invalid` (:69-78).
- **Code.** The logic is a new host-tested `no_std` crate, `captable` (the pattern of `sched` and `supervise`), wired in `kernel/src/handle/`. Minting outside the subset rule is an `unsafe trait` implemented only by `supervision.rs`, the trusted path and the powerbox glue (Tock's pattern).

### D2. Kinds

| Kind | Names | Main rights | From | Detail owned by |
|---|---|---|---|---|
| `Process`, `Job` | one process; the right to start and stop processes | INSPECT, WAIT, THREADS; SPAWN | P1 | this ADR |
| `Endpoint`, `Service` | a server's receive side; a client route to it, with a badge | RECV, MINT; CALL | P1 | this ADR |
| `Grant` | a node in the grant tree | MINT, REVOKE, INSPECT | P1 | this ADR |
| `Sched`, `Model` | the scheduler policy; the model window | READ, SET_POLICY; MAP | P1 | this ADR |
| `Component`, `Store`, `Build` | one live component's versions and ledger; the store; the build service | READ, WRITE, CONNECT | P1 | this ADR |
| `NetDest` | address and port ranges, a protocol, host-name suffixes | CONNECT, RESOLVE | P1 (fixed `llm=`, `build=`), v0.1 (fixed `pkgrepo=`, for `pkg`), P5 per app | this ADR |
| `Shm` | shared pages (D4) | MAP, READ, WRITE | P1 | this ADR |
| `Dir`, `File` | filesystem objects served by `fs`; badge (volume, object id) | READ, WRITE, ENUMERATE, CREATE, DELETE | P2 | [ADR 0004](0004-storage.md) |
| `Partition` | a `kai-data` partition | READ, WRITE | P2 | [ADR 0004](0004-storage.md) |
| `Update` | the A/B slot partitions and boot record; `updated` only | WRITE | P2 (slot writes, the update disk, `:reinstall`) | [ADR 0008](0008-updates.md) |
| `Device` | an MMIO range, an IRQ, a DMA pool | MAP, WAIT | P2 for `usbd` after ADR 0007's S07-1 (unconfined under `iommu=prefer`); P7 confined | [ADR 0007](0007-drivers.md) |
| `Display` | the screen; held only by `desktop` | MAP, WRITE | P3 | [ADR 0005](0005-graphics.md) |
| `Input` | the kernel's one input queue. READ for `desktop`'s INPUT_READ. The key sink is an `Input` handle with WRITE, the push right for ADR 0007's INPUT_PUSH, given at spawn only to the service whose `devmatch` row has `input` (`usbd`) | READ; WRITE (key sink) | P3 | [ADR 0005](0005-graphics.md), [ADR 0007](0007-drivers.md) |
| `Vm` | one micro-VM | per ADR 0006 | P6 | [ADR 0006](0006-existing-software.md) |

Server-backed kinds (`Dir`, `File`, an app's action routes) point at the server's endpoint with a badge. The kernel enforces kind, rights, subset and revocation; the server reads the badge and the rights used, as seL4 badges work.

### D3. The bitmask becomes the kind ceiling

- `Limits.caps` keeps its nine bits and their values. Each bit now means "may ever hold this kind", like Zircon job policy. A transfer to a process whose ceiling lacks the kind fails. A new kind granted at spawn gets the next free bit in claim order, whichever PR lands first: `cap::RESOLVE` (the #16 follow-up below), ADR 0007's Device, 0004's Partition, 0008's Update, 0005's Display and Input, and 0006's Vm. ADRs do not fix those numbers.
- At spawn the kernel turns the bits into startup handles, read with STARTUP, as Fuchsia gives a new process numbered handles. Ring 3 changes nothing at first.

| Bit today | Startup handle |
|---|---|
| SUPERVISE | the root `Job`, and MINT on the `system` grant root |
| SERVE | `Endpoint` for its own service name |
| CALL | one `Service` route per name in the plan's new `uses=` word, which replaces call-by-name |
| NET | `NetDest` for `llm=` (CONNECT) |
| POLICY | `Sched` (READ, SET_POLICY) |
| COMPONENT | `Component` for its own name |
| PUBLISH | `Store` (READ, WRITE) and `Build` (the `build=` destination) |
| MODEL, THREADS | `Model` (MAP); its own `Process` handle with THREADS |
| RESOLVE (new) | `NetDest` with RESOLVE only |
| none: the plan's `pkg` line | `NetDest` for `pkgrepo=` (CONNECT), the third fixed destination. `pkg` gets no cap bit ([ADR 0011](0011-packages.md)). In P5, 0011's per-host grant replaces it |

- Old calls keep working: `need(slot, cap::X)` becomes "holds a live handle of kind K with right R". Closing it drops the right for good (Capsicum's one-way rule).
- The SPAWN name rules (supervision.rs:265-283) become ceiling rules applied on every spawn path, the kernel's own included. That closes the gaps at thread_tests.rs:229 and model.rs:284.
- "Calls never chain" becomes "routes form no cycle", checked when a plan or a `kai-app 1` is accepted. No process gets a route to `shell` or `edit`, so `has_cap_by_name` (mod.rs:462-468) goes. METRICS needs a handle with INSPECT.

### D4. Shared memory: `Shm`

- One kernel object for shared pages, owned here and used by [ADR 0004](0004-storage.md) (bulk file data), [ADR 0005](0005-graphics.md) (a `Surface` is an `Shm` with a 4 KiB header page; 0005 owns `Surface`), [ADR 0006](0006-existing-software.md) (guest memory) and [ADR 0007](0007-drivers.md) (0007 owns the KRNG v1 ring format laid over an `Shm`). There is no other shared-buffer call, and 0004's fallback "shared-buffer call" is this object.
- SHM_CREATE(pages) returns a handle with MAP, READ, WRITE, DUPLICATE, TRANSFER and INSPECT. The pages count against the creator's page quota (decisions.md:69) until freed.
- SHM_MAP(handle, rights) maps read-only (MAP and READ) or read-write (MAP, READ and WRITE), never executable. SHM_UNMAP removes a mapping. Frames are freed when the last handle and mapping are gone.
- Revoking the node a holder's handle came through unmaps the pages from that holder on every CPU and maps a private zero page in their place. The holder keeps running but sees and shares nothing. Spike S01-1 M6 measures it.
- The page quota is 512 MiB per process (kernel/src/process/mod.rs:55-58). ADR 0006's guest memory needs more for its `vmm`; raising it is ADR 0006's call.

### D5. Grants and revocation

- Every handle records a grant node. There are 1,024 nodes in one tree with three roots: `kernel`, `system` (the supervisor) and `user` (the trusted path and the `powerbox` service). A node has a parent, a generation, an owner, a 24-byte label, an expiry and a life: `request`, `session` or `persistent`.
- GRANT_REVOKE bumps the generation of the node and of all its descendants: a scan of 1,024 entries at revoke time, one atomic read per check. It cancels in-flight calls made through them, using the cancel closures that watch `killed(slot)` today (syscall.rs:285-287, :375); an HTTP exchange can last 300 s (syscall.rs:18). Servers get a "badges revoked" notice from RECV_H and drop the matching sessions and open objects.
- Call-scoped grants are WIT `borrow` leases ([ADR 0003](0003-app-model.md)) and need no node. A persistent grant keeps a record, not a live node, until its app runs.

### D6. The trusted path, requests and the agent

- **The trusted path** holds the `user` root with USER: the kernel terminal now, the compositor's system layer from P3 ([ADR 0005](0005-graphics.md)). The three caller-0 checks become "the handle used has USER". RECV_H reports the badge, the rights used and the caller's pid, so pid 0 is no longer the user's identity.
- **One node per request.** Each typed line makes a node `request N` under `user`. It is revoked when the result line is written, so nothing a request was given outlives it.
- **A single tagged call** ([ADR 0002](0002-agent-actions.md)) runs from the agent core (`shell` today) through a handle duplicated under `request N` and narrowed to the objects the call names.
- **A `script`** runs in its own `scriptrun` process, holding only run-tier handles narrowed under `request N`, with ADR 0002's effect analysis and one transaction per script. The agent core starts it through a `Job` that may start only `scriptrun`. It is pooled if S01-1 M3 says so. `cc` ([ADR 0012](0012-terminal-compiler.md)) is the same kind of process: handles, no cap bit.
- **Ask tier.** No handle exists until the user's `y`. The trusted path then mints a `ONCE` handle under `request N`. Its badge names a kernel-held SHA-256 over the log entry id, the action name and the arguments exactly as the question showed them (ADR 0002). A 64-bit digest in the badge alone could be collided in about 2^32 tries, so the full digest stays in the kernel. The owner refuses a call whose digest differs. What was shown is what runs, once.
- **Never tier.** No process the agent can reach holds such a handle, so there is nothing to call.
- **Data never mints.** Only the trusted path and `powerbox` mint under `user`, and only on a keystroke, a pick or a typed request. Values carry a provenance label, `user` or `data`; [ADR 0002](0002-agent-actions.md) owns the labels and the rule that a `data` argument to an outbound, destructive or undo-less action asks. Labels can only raise a tier.
- **The model's own authority.** `llm` holds its `Endpoint`, `Model`, its `Process` with THREADS and the `llm=` `NetDest`, and no user data.

### D7. Words users see

- Each (kind, rights) pair has one word in `kabi::perm`, for example `files.read`, `files.write`, `net.connect`, `sched.set`, `sys.read`, `proc.list`, and a class: `normal` (granted at install, never asked), `user` (needs a request, a pick or a yes), `special` (Settings only) or `system` (one named holder, never the agent). v0.1 needs at most 40 words (spike S01-2 checks).
- Words appear only in `kai-app 1`, questions, Settings and the logs. The kernel never matches strings.
- Words have a dot on purpose. They are not action names, which are snake_case with no dots so scripts can call them (sheet). A dot is not a Rhai or C identifier character, so a word can never be read as a call.
- Lifetimes are `request`, `session` and `persistent`. A persistent grant needs an explicit user choice ("always").

### D8. `kai-grants 1`

- `/system/grants`, written by `powerbox` inside an [ADR 0004](0004-storage.md) transaction. It holds records, never handles, as macOS bookmarks, Android persistable URIs and Flatpak's `persistent` flag do. One line per grant, keys in this order:

```
kai-grants 1
grant 41 app=notes kind=dir object=/home/user/notes rights=read+write+enumerate from=picker life=persistent signer=- made=2026-11-02T10:14:00Z
```

- `41` is a u32 id, never reused; [ADR 0011](0011-packages.md)'s generations name grants by it. `app` is the package name. `object` is an absolute path or a host. `from` is `install`, `picker`, `request:<n>` or `settings`. `signer` is `-` until P8, then the SHA-256 of the package signer's key; a grant whose signer no longer matches is dropped. `session` lines are dropped at boot.
- `powerbox` re-derives handles under a fresh node per grant at each start. Like `kai-manifest 1`, the parser refuses unknown or reordered keys, so a new key means `kai-grants 2`.

### D9. `kai-app 1`

- Carried in the artifact's `kai.app` section ([ADR 0003](0003-app-model.md)), so in-OS builds carry it too. It is one of exactly three manifest formats (sheet): `kai-package 1`, a package's outer header ([ADR 0011](0011-packages.md)); `kai-app 1`, an app's routes (this ADR owns its grammar); and `kai-manifest 2`, runtime and lifecycle (ADR 0003). No JSON manifests. It declares kinds and routes, never objects:

```
kai-app 1
id notes
abi 1.0
use kai:files/types@0.1 scope=app-data rights=read+write+create+delete+enumerate
use kai:files/picker@0.1
expose kai:notes/actions@1.0
```

- `id` is the package name ([ADR 0011](0011-packages.md)). `abi` is the oldest `kabi::ABI` whose kinds and rights the routes need. `scope` is a keyword (`app-data`, `picked`, `home`) or, from P5, a host suffix; never a path in the user's files. Each `use` must match an import and each `expose` an export.
- Install turns the lines into the kind ceiling and the startup routes. Routes whose word is `normal` are granted with no question; a `user` word needs a user choice; a `special` word is granted only in Settings. Objects come later, from the powerbox. In the plan, native services name routes the same way with `uses=`.

### D10. The ABI and its version

- `kabi::ABI = (1, 0)`, returned by ABI_INFO with the highest call number and the table size. xtask writes a `PT_NOTE` named "KAI" holding (major, minor) into every in-tree ELF. uelf reads it, and the loader (kernel/src/process/loader.rs:88-90) refuses another major or a newer minor. Additions bump the minor; a removal or a changed meaning bumps the major.
- New calls are named here and numbered in claim order from 38 when each PR lands (decisions.md:123; RESOLVE is 37): ABI_INFO, HANDLE_DUP, HANDLE_REPLACE, HANDLE_CLOSE, HANDLE_INFO, STARTUP, GRANT_NEW, GRANT_REVOKE, ENDPOINT_MINT, CALL_H, RECV_H, REPLY_H (up to 4 handles each way), SHM_CREATE, SHM_MAP and SHM_UNMAP.
- A changed struct gets a new call and never changes in place: CALL_H sits beside CALL, whose 40-byte `CallRequest` stays pinned (kabi/src/lib.rs:286).
- Syscall numbers stay private to in-tree programs, all wrapped in user/src/sys.rs (408 lines), as Zircon keeps its numbers behind the vDSO. The public ABIs are the `kai:*` WIT packages ([ADR 0003](0003-app-model.md): semver, additions only after a release, a CI freeze check) and Linux inside the micro-VM ([ADR 0006](0006-existing-software.md)).

## Consequences

- **The agent gets a kernel-checked bound per request.** A plain request runs at once on run-tier handles; the ask tier's question is itself the grant, so there is no second permission prompt. Text the model reads cannot raise a tier, because raising one needs a handle that only the trusted path mints after a keystroke.
- **One mechanism** serves processes, apps, files, the network, devices and the agent. No later feature needs its own permission scheme.
- **Three gaps on main close:** ambient METRICS, kernel-started processes skipping the grant rules, and call-by-name.
- **It is the largest of the three options.** About 5,000 lines before v0.1 (design A's estimate, unverified), touching the syscall layer, IPC, the plan and all 12 programs in user/src/bin through user/src/sys.rs. Steps 1-6 (below) come before the rest of the P1 agent core.
- **Handles do not stop misuse inside a grant.** An injected argument can still pick a bad but undoable policy, or the wrong file in a granted folder. ADR 0002's labels and consent rules are still needed.
- **Browsers need a broad grant.** Per-site grants cannot work for the web, so the browser's micro-VM gets one network grant (founder question 3).
- **Denials are harder to debug** than a named permission. `handle_info`, a `grants` command, and `show`/`why` listing "could: read processes; set the scheduling policy (undo)" are part of the work.
- **Slots.** `scriptrun` and one process per app use process slots. [ADR 0003](0003-app-model.md) raises `MAX_PROCESSES` from 16 (kernel/src/process/mod.rs:54) to 64; S01-1 M5 reports the memory cost. Linux programs keep their file descriptors inside the guest (ADR 0006), so the `vmm` needs few handles.
- **Revocation is new code with no proof.** Zircon has no revoke call and seL4's is GPL. `captable` carries property tests (rights never grow; a revoked or stale value never resolves) and loom-style interleaving tests. A server that ignores revoke notices gets no more calls through dead handles, and a boot case kills one that keeps using them.
- **decisions.md rows.** :79 ("A coarse capability set") is superseded; :78 ("Minimal for LLM-written scripts") is met by request nodes; :71 ("ring 3 never chooses a destination") is generalised by `NetDest`. The Tock and cap-std ports rest on [ADR 0009](0009-licence.md) replacing :52 ("Borrow ideas only, clean-room") and are listed in docs/third-party-notices.md.

## What it costs to change later

| Item | Cost |
|---|---|
| Table sizes, the handle bit layout, the revocation algorithm | Cheap at any time: private behind user/src/sys.rs and comphost |
| The bootstrap (steps 1-3) | Cheap: bits remain the interface until step 4 |
| Syscall numbers and structs | Cheap: private and stamped by `kabi::ABI`; every program is rebuilt from the tree |
| `kai-app 1`, `kai-grants 1`, the words in `kabi::perm` | Lock in at v0.1. Versioned, so a v2 can sit beside v1; a renamed word needs an alias kept forever |
| The `kai:*` resource bindings | Lock in when outside apps exist (ADR 0003, founder question 7) |
| The trusted path | Most expensive: every grant's meaning rests on it |
| Swapping models | Named permissions can be built on handles later (a `use` line already is one). Retrofitting handles onto named, ambient paths means changing every call that takes a name or path, every app and the grant store. Linux shows the cost: Capsicum's Linux port stopped in 2020 and is archived, and Landlock is at ABI 11 |

## Spikes that settle it

No spike runs a real model anywhere. Boot cases use CI's LLM test double.

**S01-1 "handle-run".** On a branch, build `captable`, the kernel wiring for `Sched`, `Service` and `Shm`, request nodes, `ONCE` with its digest, and `scriptrun` running the seamless-actions `set_policy` script through handles. It runs as boot cases on GitHub Actions: KVM for absolute numbers, TCG with icount for relative ones. TCG also runs locally in WSL2.

| Measure | Pass | Fail |
|---|---|---|
| M1: extra cost of a handle check over a bit check, 100,000 cheap calls | median ≤ 100 ns (KVM) and ≤ +10% (TCG icount) | > 500 ns |
| M2: make a request node, give 8 handles to 2 processes, revoke | p99 ≤ 50 µs | > 1 ms |
| M3: `scriptrun` per request: spawn, run, exit, revoke | p99 ≤ 30 ms | > 150 ms even pooled |
| M4: 200 hand-written attack scripts: forged or stale values, a widening dup, a ceiling breach, an ask with no or a reused `ONCE`, a digest mismatch, injected text in replies | 0 succeed | any succeeds |
| M5: kernel memory for tables (at their first 256 entries) and nodes at 16 processes; also reported at 64 for ADR 0003 | ≤ 256 KiB | > 1 MiB |
| M6 (added for the sheet's `Shm`; thresholds proposed here): revoke a 256-page `Shm` mapped by 2 processes on 4 CPUs, TLB shootdown included; then write through the revoked mapping | p99 ≤ 100 µs, and 0 bytes reach the other holder | > 1 ms, or any byte arrives |

30 ms is about 1-1.5% of the sheet's 2-4 s target for a plain action under KVM (ADR 0002's budget, to be confirmed by its in-guest spike). If M3 lands between 30 and 150 ms, `scriptrun` is pooled: one long-lived process that receives `request N`'s handles and has them revoked after. If it fails pooled, scripts stay in `shell` under request nodes, and this ADR records that the Rhai engine is then the only bound on the script itself.

**S01-2 "tier corpus"** (B's permsim, re-aimed at handles and labels; shared with ADR 0002). A host crate in `spikes/` with 150 fixed cases: the 50 M8 fluency tasks, 50 new P1-P5 requests and 50 injected cases in AgentDojo's style. Pass: ≤ 5 prompts per 100 benign cases; 0 of 50 adversarial cases reach an action the user did not name without asking; ≤ 40 words in `kabi::perm`. Fail: any adversarial run (fix the rule, not the threshold), or more than 15 prompts per 100 benign cases (the request is then not enough consent, and that area needs pickers). Runs as host tests in GitHub Actions. No model.

**S03-1 "same bytes, two engines"** (WIT on wasmi) belongs to [ADR 0003](0003-app-model.md). This ADR needs only its output, a binding from WIT resources to handles, not a particular result.

## Phases that depend on it

| Phase | What it needs from this ADR |
|---|---|
| P1 agent core | Steps 1-6, before the rest of the agent core: (1) `kabi::handle`, `Rights`, the errors, ABI_INFO and the "KAI" note; (2) `captable`; (3) per-slot tables, startup handles from `caps`, `need()` on handles; (4) handle IPC, `uses=`, call-by-name retired; (5) grants, request nodes, cancel on revoke; (6) `ONCE`, `scriptrun`, `Shm`. Exit test roadmap.md:140 rests on 5-6. |
| P2 files | `Dir`, `File` and `Partition` (ADR 0004); the `Update` kind (ADR 0008); `Device` for `usbd` after ADR 0007's S07-1; the `powerbox` service; `kai-grants 1` on disk |
| P3 desktop | `Display`, `Input` and `Surface` on `Shm` (ADR 0005); the key sink for `usbd` (ADR 0007); the picker and the USER root in the compositor's system layer; `kai-app 1` from artifacts |
| P4 built-in apps | Settings "what each app can do" in the words of D7; revoke from Settings and from `undo` |
| P5 networking | `NetDest` per app (Tock port), RESOLVE as a right on it, HTTP and connect on handles, ADR 0011's per-host grant in place of `pkgrepo=`, `psl` if approved |
| P6 existing apps | the `Vm` kind and one network grant per VM (ADR 0006) |
| P7 real hardware | AMD-Vi confinement of `Device` holders and `iommu=require` (ADR 0007) |
| P8 production | signer-bound grants, fuzzing of every parser here, a security review of the trusted path |

## Changes to recently merged and open work

- **#15 seamless actions (merged).** One small follow-up PR:
  - `shellcore/src/tiers.rs:35-44`: add `("fixed", Tier::Run)` and `("floor", Tier::Run)`. They are registered outside the table (shellcore/src/script.rs:190-191).
  - `shellcore/src/tests/actions.rs:37-50`: check every function the strict engine registers (shellcore/src/script.rs:190-192), not only `API`, and let the two pure helpers pass the second loop.
  - `shellcore/src/actions.rs:104-109`: the audit line gains the D7 word: `script 4 set_policy (run, sched.set): policy round-robin (was interactive)`. A const map beside `TIERS` gives `sys.read` (cpus, memory, uptime_ms, get_policy), `proc.list` (processes, threads) and `sched.set` (set_policy).
  - Why: every callable then has a row, and the audit log speaks the words Settings will show. At P1, ADR 0002's `agentabi` rows carry `needs: (Kind, Rights)`, and `set_policy` runs on a per-request `Sched` handle instead of the shell's standing POLICY bit.
- **#16 DHCP, DNS and ping (open, 131aa77).** Before it merges, keep call 37 but gate it on a new bit:
  - kabi/src/lib.rs: `cap::RESOLVE` at the next free bit, added to `GRANTABLE` (:108 on main); kabi/src/call.rs:133 on the branch: "Needs `cap::RESOLVE`".
  - kernel/src/process/syscall.rs:159-160 on the branch: `need(slot, cap::RESOLVE)` in place of `cap::NET`.
  - supervise/src/lib.rs:126-138: parse `resolve`.
  - kernel/src/net_tests.rs:195 and :202 on the branch: `netcheck` gets `cap::RESOLVE`, plus a case that NET alone is refused.
  - The branch's decisions.md row "DNS resolver and RESOLVE (37)": "gated by `cap::RESOLVE`".
  - No plan line grants it; `llm` keeps its bits, `serve+net` plus `model+threads` when a model is loaded (kernel/src/llm.rs:75, :79).
  - Why: as written, NET lets `llm` look up any name, and a lookup of `<data>.attacker.example` carries data out. `:resolve` and `:ping` are typed kernel commands and are unaffected. In P5 the bit becomes the RESOLVE right on a `NetDest`.
- **#17 terminal look (merged).** Keep `discard_pending` (kernel/src/terminal.rs:109) and the `answer` refusal (:137). Found by reading the code, not run: ring-3 text can take the prompt and question styles. Row kinds come from a row's text alone (termcore/src/style.rs:1-3, :58-78). A ring-3 row such as `[app] run script 9? y/n> ` matches `prompt_width` (:112-120) and is painted as a prompt. And a WRITE piece is 160 characters after a `[name] ` prefix of up to 27 (kernel/src/process/syscall.rs:15, :216-224), while the console guarantees only 162 columns (kernel/src/console/layout.rs:17). The grid wraps the excess onto a new row (termcore/src/grid.rs:273-276), whose text ring 3 chose and which may start with `? `. A follow-up PR:
  - termcore/src/style.rs: `Kind::of` returns `Log` for any row that starts with `[`, before every other check, with tests for both cases above. No kernel line starts with `[` (kernel/src/process/syscall.rs:198-200).
  - kernel/src/process/syscall.rs:216-224: cut pieces so `[name] ` plus the piece fits in `terminal::PIECE_CHARS`, so ring-3 text never starts a row. Model text in `ai:` lines has the same overflow (kernel/src/terminal.rs:316, 4 + 160 characters); cut it to `PIECE_CHARS - 4`.
  - Why: the ask tier and the USER root rest on ring 3 never drawing a question or a prompt.
- **#19 USB input (open, 0717caa).** No change now. Keys become PS/2 set 1 scancodes in the i8042 ring (kernel/src/usb.rs:1-5 on the branch), so `discard_pending` still applies. When xHCI moves to a ring-3 `usbd` (ADR 0007, in P2 after S07-1), its keys must still enter that one kernel path, through the key sink (an `Input` handle with WRITE, D2) that only `usbd` holds. That makes `usbd` part of the trusted path for ADR 0007's review. The `Hal` trait (xhci/src/hal.rs:102 on the branch) is where `Device` handles plug in.
- **#18 virtio-gpu (open, af7635d).** No change now. In P3 its buffers are `Shm` surfaces, and a top layer is reserved for the trusted path (ADR 0005).
- **#13 parser hardening (merged).** When they land, add hostile tests with testkit/hostile.rs for the `kai-grants 1` and `kai-app 1` parsers and the RECV_H message, and extend uelf/tests/hostile.rs to mutate the "KAI" note.
- **#14 faster decode (merged).** None.

## Open questions for the founder

1. When a plain request opens something ("go to youtube", "open report.txt"), should the app keep that access after the request ends, or lose it by default with an "always" choice? Keeping is smoother; losing is safer. This ADR loses it until you decide.
2. Should "always" grants an app has not used for about 90 days reset on their own? Android resets permissions of apps unused for "a few months".
3. The browser runs inside a Linux micro-VM (ADR 0006). Do you accept that the VM gets broad internet access at install, after one question, since per-site grants cannot work for the web?
4. Is it fine that the AI can list and read any file in your home folder without asking (roadmap P2), given it can never send data out or delete without asking?
5. Licence: may we copy WASI's WIT text, which is under the W3C Community Contributor License Agreement, or only its shapes? And do seL4 (GPLv2), Genode (AGPLv3) and Flatpak (LGPL-2.1-or-later) stay ideas only, with no code?
6. Licence: P5's host checks would embed the Public Suffix List, which is MPL-2.0, through `psl` 2.1.244 (the crate itself is MIT/Apache-2.0). MPL is now "ask". Approve it, or keep our own short list of suffixes?
7. Will outside developers ship apps before v1.0? That decides when `kai-app 1` and the `kai:*` app ABI freeze. ([ADR 0003](0003-app-model.md) asks the same.)

Crates this ADR adds are in the free class and need no question; each PR lists them: `bitflags` 2.13.2 (MIT OR Apache-2.0) as a direct dependency of `kabi`; the Tock files (Apache-2.0 OR MIT) and the cap-std algorithm (Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT) as ports with their headers; `psl-types` 2.0.11 (MIT/Apache-2.0) only if question 6 is yes.

## Sources

External, read on 2026-10-21 unless a date is given:

- Zircon handles: https://fuchsia.dev/fuchsia-src/concepts/kernel/handles. `zx_handle_duplicate` ("strictly lesser"; `ZX_ERR_INVALID_ARGS`): https://fuchsia.dev/fuchsia-src/reference/syscalls/handle_duplicate (updated 2025-03-04). Rights, 25 named, `ZX_RIGHTS_BASIC`: https://fuchsia.dev/fuchsia-src/concepts/kernel/rights (updated 2025-03-22). vDSO: https://fuchsia.dev/fuchsia-src/concepts/kernel/vdso. Routing: https://fuchsia.dev/fuchsia-src/concepts/components/v2/capabilities (updated 2026-10-12). Licence: zircon/kernel/LICENSE (MIT).
- seL4 API (`seL4_CNode_Mint`, `seL4_CNode_Revoke`): https://docs.sel4.systems/projects/sel4/api-doc.html; release 16.0.0 (2026-07-22), https://github.com/seL4/seL4/blob/master/LICENSE.md (kernel GPLv2).
- Genode Foundations 25.05: https://genode.org/documentation/genode-foundations/25.05/architecture/Recursive_system_structure.html; code at https://codeberg.org/genodelabs/genode. Licence (AGPLv3 with a linking exception) read in LICENSE on the archived GitHub mirror, https://github.com/genodelabs/genode (last push 2026-05-20).
- capsicum(4), the FreeBSD 15.1 man page dated 2026-01-23: https://man.freebsd.org/cgi/man.cgi?query=capsicum&sektion=4. Landlock ABI 11: https://docs.kernel.org/userspace-api/landlock.html. Capsicum for Linux, archived, last push 2020-06-03: https://github.com/google/capsicum-linux.
- Redox namespace manager (2026-03-17): https://redox-os.org/news/nlnet-cap-nsmgr-cwd.
- Tock release-2.3.0 (2026-10-08): https://github.com/tock/tock/blob/master/kernel/src/capabilities.rs and capsules/extra/src/net/network_capabilities.rs (173 lines on master, SPDX Apache-2.0 OR MIT).
- crates.io API: bitflags 2.13.2 (2026-09-10), cap-std 4.0.3 (2026-08-20), psl 2.1.244 (2026-10-21), psl-types 2.0.11 (2022-08-10), wit-parser 0.262.0 (2026-10-13), wit-bindgen 0.63.0 (2026-10-14), wasm_component_layer 0.1.18 (2024-08-02, still the newest), regorus 0.13.0 (2026-10-14), cedar-policy 4.14.0 (2026-10-17), biscuit-auth 6.0.0 (2025-07-16), all at https://crates.io/crates/<name>/<version>. Their `no_std` status is as design B and the research read it (not re-read).
- Public Suffix List (MPL-2.0 header): https://publicsuffix.org/list/public_suffix_list.dat.
- wasmi component model: https://github.com/wasmi-labs/wasmi/issues/897 (open since 2024-01-16) and /issues/1665 (the maintainer's reply of 2025-09-20 names `wasm_component_layer`; closed 2025-09-23).
- WASI v0.3.2 (2026-10-13) filesystem `types.wit` and `LICENSE.md` (W3C CLA): https://github.com/WebAssembly/WASI. WIT `own` and `borrow`: https://component-model.bytecodealliance.org/design/wit.html.
- Powerbox: https://docs.sandstorm.io/en/latest/developing/powerbox/. macOS: https://developer.apple.com/documentation/security/accessing-files-from-the-macos-app-sandbox. Android: https://developer.android.com/guide/topics/permissions/overview and https://developer.android.com/training/permissions/requesting (updated 2026-10-16; the "a few months" reset). Flatpak documents portal: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Documents.html (xdg-desktop-portal 1.22.1, 2026-06-17). Hubris: https://hubris.oxide.computer/reference/.
- Agent security: CaMeL, arXiv 2503.18813 v2 (2025-06-24); design patterns, arXiv 2506.08837 v3 (2025-06-27); FIDES, arXiv 2505.23643 v2 (2025-09-03); Progent, arXiv 2504.11703 v3 (2026-05-14); AgentDojo, https://github.com/ethz-spylab/agentdojo (MIT). Code licences from GitHub: google-research/camel-prompt-injection (Apache-2.0), sunblaze-ucb/progent (MIT), microsoft/fides (MIT).

Repo, on main at ec5ecff: kabi/src/lib.rs, kabi/src/call.rs, kabi/src/plan.rs; kernel/src/process/{syscall,mod,supervision,loader}.rs; kernel/src/model.rs, kernel/src/thread_tests.rs, kernel/src/terminal.rs, kernel/src/llm.rs, kernel/src/selftest.rs, kernel/src/console/layout.rs; termcore/src/{style,grid}.rs; uelf/src/lib.rs; supervise/src/lib.rs; shellcore/src/{tiers,actions,script}.rs, shellcore/src/tests/actions.rs; user/src/bin/shell.rs, user/src/sys.rs; editcore/src/service.rs; livecore/src/{host,artifact}.rs; testkit/hostile.rs; wasmhost/Cargo.toml, xtask/Cargo.toml; Cargo.lock (:75-78, :1153-1156, :1190-1202); docs/decisions.md (:52, :69, :71, :74, :78, :79, :111, :123), docs/roadmap.md (:129-131, :139-147, :187), docs/research/compiler-in-terminal.md (:46, :249). Branches: origin/dane/net-dhcp-dns-b5bdb7 (131aa77), origin/dane/usb-input-83eb7c (0717caa), origin/dane/virtio-gpu-a5daaa (af7635d).
