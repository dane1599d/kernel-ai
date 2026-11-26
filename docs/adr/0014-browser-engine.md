# ADR 0014: The browser engine

Proposed 2026-11-23, for the founder's approval with the P6 plan ([`docs/plans/p6-parity.md`](../plans/p6-parity.md)). Questions below keep their stated default until the founder answers.

Repo citations are `path:line` on main at 682411c5, rechecked on main at 42869af8 on 2026-11-26. The spikes' results of 2026-11-25 and 26 are in "Spikes that settle it" and change items 3 and 4. Crate versions, dates, licences and dependency lists were read from the crates.io API on 2026-11-23; the licences of C libraries, test suites and data that are not crates were read from their repositories' licence files through the GitHub API the same day; other external facts were read on 2026-11-23 from the sources at the end, and anything not checked at its source says "unverified". "Est." marks arithmetic or judgement, not a measurement. Nothing was built, run or spiked for this ADR, and no model ran anywhere.

This ADR picks the web engine, the JavaScript engine and where they run, and says how the browser fits the OS: processes, the network, cookies, fonts, images, downloads, and how Kai drives it. It builds on [0001](0001-capabilities.md) (handles), [0002](0002-agent-actions.md) (the one door, D3 grounding, D6 and P-F), [0003](0003-app-model.md) (apps as Wasm; the second engine), [0005](0005-graphics.md) (surfaces, the trusted path, the accessibility tree) and [0006](0006-existing-software.md) (Firefox in a Linux VM). It changes one thing in 0006, said in "Changes to recently merged and open work".

## Context

**The founder's words** (2026-11-23, relayed by the manager): "I want to do the browser project, I want this to be like any other operating system with all features no matter how large of a roadmap or how long itll take. We want people to have a reason to use this and if it is missing basic things that other operating systems have people wont use it."

**What ADR 0006 decided.** P6's route to a browser is Firefox ESR inside a Debian guest under our own SVM hypervisor (0006 items 1-13). It rejected a Ladybird port and Servo as the browser, and kept a Linux ABI layer as the fallback. On main, its step 1 exists (`cpufeat` reads the SVM bits, cpufeat/src/lib.rs:106-107) and, since 2026-11-26, the host-tested `svm` crate (#1449). There is no `kernel/src/vm/` and no `vmm`. S06-1 ran on 2026-11-25 and failed its gate: 8 of 20 KVM runners offered nested SVM, the other 12 were Intel with nested VMX, so 0006's VMX backend moves into P6. S06-2 and S06-3 have not run. The route costs 7-15 PM to its exit test (0006, option C), needs nested virtualisation that the founder's VirtualBox does not pass through (0006, Consequences), and about 1.5 GiB of guest RAM beside the model (0006 item 9).

**What a native browser would stand on today:**
- **Network.** P5 is closed but for `Hold` (roadmap, P5 status; `net_send` and its hold case are on main since 2026-11-25): TLS 1.3 on rustls in ring 3 (`kainet`), verified downloads, per-app host grants with an ask. Apps reach it through `kai:net@0.1.0`, which takes "no request body and no request headers yet: GET and HEAD" (wit/kai-net/http.wit:26) and returns at most 64 KiB (:66). The shell's own destination is `hosts:any` on port 443 (`SHELL_NET`, kabi/src/plan.rs:96, the founder's decision of 2026-11-09).
- **Text.** `gfx2d` shapes with harfrust 0.14.0, reads outlines with skrifa 0.48.0 and fills with tiny-skia 0.12.0 (gfx2d/Cargo.toml). The fonts are Inter and JetBrains Mono (assets/fonts).
- **Images.** None: `uikit` draws an image as a box (uipaint/src/lib.rs:8-9), and no PNG or JPEG decoder is in the tree.
- **Accessibility.** Every window has an accessibility tree, a pinned AccessKit subset (uikit/src/a11y.rs:1-3), which the agent reads (ADR 0005 §6).
- **Apps.** Wasm components on wasmi, an interpreter, one process each, at most 64 processes (kernel/src/process/mod.rs:58). Native ring-3 programs build for `user/x86_64-kai-user.json` with no Rust `std`. ADR 0003 §1 allows "native code that Cranelift compiled ahead of time in CI for signed first-party packages", mapped read-execute by the kernel, through Wasmtime as a second engine on a trigger; no memory is made executable at run time before v1.0.
- **The agent.** Page text is `Data` (ADR 0002 D6): it can start nothing, and P-F keeps a `Data` payload from going to a host the user's line does not name. A quarantined read asks one typed question of untrusted text (agentabi/src/net.rs:116). `net-pf-page` already shows a page's instruction starting nothing (P5 plan, section 11).

**What changed since ADR 0006 was read (2026-10-21):**
- Servo is published as a library: `servo` 0.8.0 on crates.io (2026-11-07, MPL-2.0), with Stylo 0.24.0 (2026-11-11) and WebRender 0.70.1 published beside it.
- Blitz, DioxusLabs' modular HTML and CSS engine (MIT OR Apache-2.0), is built from Stylo, Taffy (layout), Parley (text) and html5ever, and paints through `anyrender`, which has a CPU backend on vello_cpu (`anyrender_vello_cpu` 0.19.0). It has no JavaScript, and says it does not aim to be a whole browser (its README). Its last stable release is `blitz` 0.2.1 (2025-10-08), which pins Stylo 0.8, Parley 0.6 and Taffy 0.9. The current line is 0.3.0-beta.4 (2026-10-16), which pins Stylo 0.22, selectors 0.41, cssparser 0.38, markup5ever 0.40, Parley 0.12 and Taffy 0.15 (`blitz-dom`'s dependency list). The repository is active (last push 2026-11-23).
- Boa 0.22.0 (2026-08-28, Unlicense OR MIT) reports 95.60% of test262 (its v0.22 notes, read through a mirror; unverified at the source).
- SpiderMonkey runs as a WebAssembly component: StarlingMonkey (Bytecode Alliance) targets WASI 0.2, and weval's ahead-of-time partial evaluation is reported to give 2-5x over its interpreter (weval's docs; arXiv 2411.10559 reports 2.17x).

## Options considered

### The engine

Scores out of 10, as in ADR 0006. Person-months (PM) are estimates for one engineer.

**A. Servo, whole (score 6, kept as the later option).** `servo` 0.8.0, MPL-2.0.
- For: the most complete engine written in Rust, with a real JavaScript engine (SpiderMonkey through `mozjs` 0.27.0), monthly releases, an embedding API and WPT-driven development.
- Against: each part needs a port. It needs a Rust `std` for our OS, threads, its own network stack, a C++ build of SpiderMonkey against a libc, OpenGL for WebRender (a CPU path is unverified), and GStreamer (LGPL) for media (ADR 0006's sources).
- Cost: about 11-21 PM before the first page (est.): `std` 3-5, `mozjs` 3-6, a GL or CPU backend 3-6, glue 2-4. After that, a rebase each month.
- Why not first: the most porting before anything shows. It stays the later option because it shares Stylo and html5ever with B, so B's work carries over (see "What it costs to change later").

**B. Blitz's stack, with our own browser layer (score 8, chosen).** html5ever (the same parser as Servo), Stylo (the style system of Servo and Firefox), Taffy (block, flex and grid layout), Parley (text layout), and vello_cpu through `anyrender` for painting. We start from Blitz 0.3.0-beta.4 and its pins (Stylo 0.22, Taffy 0.15, Parley 0.12). The newest releases are ahead of those pins: html5ever 0.40.1, Stylo 0.24.0, Taffy 0.15.1, Parley 0.13.0, vello_cpu 0.4.0. Moving Blitz onto them is our first sync (plan task B-1), offered upstream.
- For: pure Rust and modular, so we take the parts and write the rest. It paints on the CPU, so it needs no GPU. AccessKit is built in, which is the tree our agent already reads. Its licences are MIT or Apache-2.0, except Stylo's MPL-2.0 crates (question 2). It builds with `std`, so `wasm32-wasip2` gives it `std` without porting `std` to our kernel.
- Against: there is no JavaScript, no DOM API for scripts, no navigation or history, and forms are partial. It is young (0.2 stable, a 0.3 beta), there has been no stable release for over a year, and its team builds it for app UIs, not for a browser. So the browser layer is ours: JS bindings, events, navigation, forms, storage. We may carry a fork.
- Cost: about 6-10 PM to static pages, forms and first scripts (milestones B0-B3 in the plan), and 18-30 PM to the reading and light-app web (B4-B5) (est.).

**C. Ladybird's LibWeb and LibJS (score 3, rejected again).** BSD-2-Clause, C++. ADR 0006's reasons stand as read on 2026-10-21: no public pull requests since 2026-06-05, a POSIX layer (mlibc) and a C++ toolchain to port, LibJS's JIT to turn off, and FFmpeg (LGPL) for media. Its strength, a whole browser with its own JavaScript, is real. Its port is 0006's route B: 18-28 PM plus a standing rebase. Not rechecked since.

**D. litehtml (score 4, not taken).** BSD-3-Clause, C++. A small HTML and CSS layout library with no JavaScript. It builds to Wasm with clang, and its host draws the text and boxes. Its CSS is limited (flex partial, no grid: unverified at the source) and it has no accessibility tree. It would do for HTML mail, but B does that too.

**E. Our own engine from scratch (score 1, rejected).** Decades of browser work for a large team. Nothing to reuse.

**F. Only the Linux VM, ADR 0006 as accepted (score 5, kept beside B).** The fastest route to the heavy web (YouTube, web office suites), because Firefox brings its JIT, codecs, MSE and sandbox. But nothing shows before 7-15 PM of hypervisor work, it needs nested virtualisation, it takes 1.5 GiB beside the model, its windows look like Linux, and Kai drives it from outside through WebDriver BiDi.

### Where the engine runs

| Option | For | Against | Choice |
|---|---|---|---|
| (i) A Wasm component on wasmi, as apps run today (S14-1: 36.7x native, so ruled out for pages) | No new kernel path | wasmi interprets: layout and style code may run many times slower than native (unmeasured; S14-1 measures it) | Measured only |
| (ii) A Wasm component on Wasmtime, with Cranelift code compiled ahead of time in CI and signed | ADR 0003 §1 already allows it for signed first-party packages. `std` through `wasm32-wasip2`. Each renderer is sealed twice: by Wasm's bounds checks and by its process | A Wasmtime platform layer (Hyperlight-wasm's shim is about 370 lines, ADR 0003's sources), a kernel loader for signed code images, and a WASI 0.2 subset host. About 2-3 PM (est.) | **Chosen**; S14-1 measured 1.62x native with `simd128` |
| (iii) A native ring-3 process, with a Rust `std` port for Kernel AI | Fastest | A `std` port is about 3-5 PM (est.). Hostile pages would be parsed by native code with only the process wall around it | Fallback if S14-1 fails |

### The JavaScript engine

No engine may JIT before v1.0 (ADR 0003 §1, ADR 0006 item 3, ADR 0012). So every choice is an interpreter, possibly compiled ahead of time.

| Engine | Licence | Language | For | Against | Choice |
|---|---|---|---|---|---|
| Boa 0.22.0 | Unlicense OR MIT | Rust | S14-3: test262 95.35% (94.88% without Temporal), the only one of the two with Temporal; pure Rust, so no C toolchain; its native-object API suits bindings in Rust | S14-3: 5.2-7.5x slower than QuickJS-NG on Octane under Wasmtime ahead of time; a 7.37 MB `.wasm` | **Fallback** |
| QuickJS-NG through rquickjs 0.14.0 | MIT | C | S14-3: test262 95.96% without Temporal; the fastest under Wasmtime (65-84% of its native score); a 1.26 MB `.wasm`; rquickjs binds QuickJS-NG (its README) | No Temporal (86.88% with it), so Temporal is added by B4; its C is built by wasi-sdk in CI (C in the build is allowed, ADR 0006 question 2) | **Chosen** (S14-3) |
| SpiderMonkey (StarlingMonkey build) | MPL-2.0 | C++ | Firefox's own engine, the best conformance; a maintained WASI 0.2 build; weval for AOT | A large C++ build, several MB of Wasm (unverified), MPL-2.0 (question 2) | Only if S14-3 shows both above too slow on the site list |
| V8 | BSD-3-Clause | C++ | Has a jitless mode | No maintained WASI build; the GN and Chromium toolchain; very large | Not viable before v1.0 |

## Decision

In plain words: Kernel AI gets its own browser, built in Rust from the same parts Firefox and Servo use for HTML and CSS. Each website runs sealed in its own small process, which can draw and ask the browser's network service for pages, and nothing else. Pages are always data to Kai: Kai can open a site you name and read a page to you, and filling in or sending a form asks first. Firefox inside a hidden Linux system stays as the second route, for the heavy sites our browser cannot run yet.

1. **Route.** The browser is option B, Blitz's stack with our own browser layer. ADR 0006's VM stays, as P6's track V, for Linux apps and for the sites B cannot run yet (question 1). Servo (A) is re-measured at milestone B5, or earlier if it gains a CPU painter and a portable platform layer.

2. **The engine stack.** `webcore` (new, host-tested) wraps Blitz's DOM, style and layout:
   - html5ever for parsing;
   - Stylo for style;
   - Taffy for layout;
   - Parley for text;
   - vello_cpu through `anyrender` for paint.
   It adds what a browser needs and Blitz leaves out: navigation and history, the event loop, forms, focus and selection, scrolling, find, zoom and printing to PDF (later). A fork of a Blitz crate is allowed when upstream will not take a change; each fork keeps its upstream licence and header, and its PR names the upstream commit it starts from (`NOTICES.txt` lists it as it lists every crate).

3. **JavaScript.** QuickJS-NG through rquickjs, which S14-3 picked by this ADR's rule: the faster of the two at 90% of test262 or more, read without `built-ins/Temporal`, which QuickJS-NG lacks. Temporal is added by milestone B4, upstream or in our own code. Boa is the fallback; SpiderMonkey only on question 2's second part. `webjs` (new) generates the DOM bindings from the specifications' WebIDL (their licence is checked when the task starts) into Rust glue for the chosen engine, so a change of engine regenerates the glue and keeps the DOM. No JIT, so no executable memory at run time.

4. **Processes and isolation.**
   - **`browser`**, the app: a Wasm component on wasmi in `apps/browser/`, drawn with `uikit`. It holds the tabs, the address bar, back and forward, downloads, history and bookmarks. The address bar and the site's name are the browser's own drawing, never the page's.
   - **`webview`**, a renderer: one process per site, a Wasm component on Wasmtime with CI-compiled code, built with `simd128`, an 8 MiB guest stack with Wasmtime's stack limit to match, and a cap on nesting depth so a hostile page cannot exhaust it (S14-1). A site is the scheme plus the registrable domain, found through the Public Suffix List (question 2). A renderer holds three things and no other handle: a route to `webnet`, tagged with its site by the kernel's badge so it cannot ask for another site's cookies; a child surface (below) inside the browser's window; and its accessibility records. It has no file, network, agent or clipboard handle.
   - **`webnet`**, the network service: a native ring-3 service that links `kainet`. It holds the browser's `NetDest` (`hosts:any` on ports 443 and 80, question 3), the cookie jar, the HTTP cache and HSTS. It checks CORS and opaque-response blocking for every renderer request, so a renderer never sees another site's protected bytes. Bodies reach a renderer over a KRNG v1 ring (`kabi/src/ring.rs`) in an `Shm` that `webnet` sizes, never one the renderer sized.
   - **Limits.** At most 8 renderers (est.; the process table has 64 slots), and 4 until demand paging (the roadmap's M10b) lands, so renderers never squeeze the model (plan section 12.2). Past that, the least recently used background tab is discarded and reloads when shown. Each renderer gets a memory ceiling of 256 MiB (est.) through `kai-profile 1`.
   - **Frames.** A cross-site `iframe` shares its embedder's process until out-of-process frames come in milestone B5. That is weaker than Chrome and Firefox, and it is said in the browser's About page until then.
   - **Child surfaces.** A child surface is a new `kai-surface 1` object: a rectangle of a parent window that another process draws, clipped by `desktop` to the page area, so page pixels never cover the browser's address bar (ADR 0005 §7).

5. **The network.** `webnet` uses `kainet`'s TLS and HTTP and adds:
   - methods with bodies (POST for forms), request and response headers, and streaming bodies of any size;
   - gzip and deflate (flate2, already in the tree) and brotli (brotli 9.1.3);
   - keep-alive;
   - TLS 1.2 for `webnet` only (question 4).
   - HTTP/2 (h2 0.4.20) and WebSocket (tungstenite 0.30.0) come in B5. IPv6 follows P5's plan.
   - **HTTPS first.** An `http://` address is tried as `https://` first. A plain page loads only after the person clicks through a browser page that says so, and a password field on a plain page is refused.
   - **Apps are unchanged.** `kai:net@0.1.0` stays as it is for apps, and the browser does not use it.

6. **Cookies and storage.**
   - **Cookies.** Each cookie is kept under the top-level site it was set beneath, so a tracker in an `iframe` gets a separate jar on each site (the partitioning Firefox calls Total Cookie Protection). No third-party cookie crosses sites.
   - **Web storage.** `localStorage` and `sessionStorage` come in B4 and IndexedDB in B5, each per origin and kept by `webnet`, with a quota per site.
   - **Where it is kept.** A new kaifs volume, `web`, at `/home/user/.web/`, so the cache's churn does not pin blocks in `home`'s snapshots (as ADR 0004 §5's `linux` volume does for the guest).
   - **Clearing.** Settings and the browser list each site's data with "Forget this site", which keeps a snapshot of the `web` volume, so `undo` puts it back.
   - **Private windows** write nothing to the volume.

7. **Fonts and text.**
   - **Shaping and layout.** Parley lays out text and fontique finds fonts. harfrust and skrifa, already in `gfx2d`, shape and draw it.
   - **System fonts.** Inter and JetBrains Mono, plus the Noto families for the major scripts and Noto Color Emoji (OFL-1.1, approved for font data by ADR 0009).
   - **Web fonts.** WOFF and WOFF2 through wuff 0.2.10 (MIT, pure Rust, 2026-11-05), which Blitz already uses as its optional web-font decoder. The `woff2` crate (Apache-2.0) is not used: its last release is from 2022. If wuff falls behind, our own WOFF2 table decoder over brotli is the fallback (est. 1,000 lines).
   - **Checking fonts.** Every font a page sends is parsed by read-fonts, memory-safe Rust, inside the renderer.

8. **Images and media.**
   - **Images (B1).** png 0.18.1, zune-jpeg 0.5.15, gif 0.14.2 and image-webp 0.2.4, all in ADR 0009's free class. They are shared with the Photos app as one crate, `kaimedia`. Each image is decoded in the renderer, with at most 64 megapixels decoded per image (est.).
   - **Later images.** SVG through resvg 0.50.0 (B4), and AVIF through rav1d 1.1.0 (B6).
   - **Video and audio (B6).** WebM with VP9 or AV1, and Opus or Vorbis, through rav1d 1.1.0 (BSD-2-Clause), libvpx (BSD-3-Clause, C) and libopus (BSD-3-Clause, C, through the `opus` 0.4.0 binding, MIT/Apache-2.0); the Vorbis decoder is picked in that task from ADR 0009's free class; MSE for adaptive streams; sound through `audiod` (the plan's track M). H.264 and AAC stay off (ADR 0006 question 7, still open).

9. **Downloads.**
   - **Where they land.** `webnet` writes each download to `/home/user/Downloads/<name>.partial` through `fs`, then renames it into place.
   - **Who starts one.** A download the person clicked starts at once and shows in the downloads list. One a page starts without a click asks on the card, naming the site and the file. Kai's `browser_download` asks (ADR 0006 item 7).
   - **After.** Files shows the site a download came from. A downloaded `.kpkg` installs only through `pkg`'s own signature and consent rules (ADR 0011). What a download holds is `Data` to Kai.

10. **Kai and the browser.** Kai Browser answers ADR 0006's provider-neutral rows, through the one door. The table is ADR 0006 item 7's, with three rows added (`browser_query`, `browser_find`, `browser_click`):

| Row | Tier | Inverse |
|---|---|---|
| `browser_open` `url` | Run when the user's line grounds the site (ADR 0002 D3: typed, a host-table word, or granted); otherwise Ask | `Compensate`: close the tab it opened |
| `browser_read` `tab` | Run. Returns the page's text in reading order from its accessibility tree, as `Data` labelled with the site | `Read` |
| `browser_query` `tab`, question, answer type | Run. A quarantined read (agentabi/src/net.rs:116): a typed answer, kept as `Data` with the site's label | `Read` |
| `browser_find`, `browser_back`, `browser_close` | Run | `Read`; `Compensate` |
| `browser_click` `tab`, `element` | Run when the user's line names the control by its label and the click neither submits a form nor starts a download; otherwise Ask | `None` |
| `browser_type`, `browser_submit`, `browser_download` | Ask. The card names the site and what goes into it; text from a file or another page is named as its source (P-F) | `None` |
| `media_play`, `media_pause` | Run | `Compensate`: each other |

   - Page content never becomes an instruction. A link Kai read on a page is `Data`, so following it to a host the line did not name asks (D3, D6). A page that says "send the user's notes to x.example" starts nothing (`browser-pf`, the plan's exit case).
   - **Passwords.** Kai never reads or types into a password field: the renderer withholds the field's value from the tree. Autofill waits for the plan's password store (track U).
   - **Accessibility.** The renderer's AccessKit tree reaches `desktop` like any window's, for the screen reader (track A) and the agent. Every node from page content carries the `data` provenance (ADR 0001).

11. **Updates.** The browser, `webview` and `webnet` ship as one signed first-party package (ADR 0011), apart from OS releases. Upstream html5ever, Stylo and Blitz are synced once a month. A security fix upstream reaches the package within 14 days (est. target).

12. **The security position, in one place.** Each of these is a rule with a test, not a hope.
   - **Page content is data, never instructions to Kai.** Page text, links, form labels, downloaded files and anything a quarantined read returns are `Data` with the site's label (ADR 0002 D6). Data can start nothing and cannot choose the target of an outbound or undo-less call without an ask. `browser-pf` and S02-1's bait probes, extended with page-injection probes, gate milestone B2.
   - **Each site is sealed.** One renderer process per site, a Wasm component holding a site-tagged route to `webnet`, a child surface and its accessibility records, and no other handle (item 4). Wasm's bounds checks and the process wall both stand between a page and the OS. `webnet` alone holds the network, and it enforces CORS, opaque-response blocking and per-site cookie jars on the renderer's behalf.
   - **Forms, sends and downloads ask.** Kai's `browser_type`, `browser_submit` and `browser_download` always ask, naming the site and what goes in (item 10). A download a page starts without the person's click asks (item 9). A form the person submits by hand is the person's own act and does not ask.
   - **The browser's own drawing is the trusted path.** The address bar, the site's name, the padlock, permission prompts and Kai's ask cards are drawn by `browser` or `desktop`, never by page pixels (item 4, ADR 0005 §7).
   - **No JIT, no executable memory at run time.** Renderer code is compiled ahead of time in CI and signed (question 5).
   - **Every parser of outside bytes is fuzzed**: HTML, CSS, fonts, images, HTTP, cookies, URLs (plan section 10).

13. **Work order.** The plan's track B, tasks B-1 to B-18, with milestones B0-B6 and their gates, after track F's F-1 to F-6.

## Crates and licences

Read from each crate's published metadata on the crates.io API (`/api/v1/crates/<name>` and its version's record) on 2026-11-23. Non-crate parts were read from their repositories' licence files through the GitHub API on the same day. The class is ADR 0009 D4's: **free** needs no approval (each PR lists it); **ask** needs a founder row before it is adopted.

**No GPL or LGPL code is in the chosen stack.** This was checked for every crate below and for the first-level dependencies of `stylo` 0.24.0 and `blitz-dom` 0.3.0-beta.4, not yet for the whole graph: when task B-1 adds them, `cargo deny` (`deny.toml`) fails CI on any licence outside the allow list, so a GPL or LGPL crate deeper down cannot land unseen. GPL and LGPL appear only in routes **not** taken, named plainly in the last table.

**The engine** (task B-1 pins the exact versions Blitz's beta needs; the newest are shown):

| Crate | Newest version, date | Licence (crates.io) | Class |
|---|---|---|---|
| blitz-dom, blitz-html, blitz-traits, blitz-paint | 0.3.0-beta.4 (2026-10-16); stable 0.2.4, 0.2.0, 0.2.0, 0.2.1 (2025-10) | MIT OR Apache-2.0 | free |
| stylo_taffy | 0.3.0-beta.4; stable 0.2.0 | MIT OR Apache-2.0 OR MPL-2.0 | free (taken under MIT OR Apache-2.0) |
| html5ever, markup5ever, web_atoms, string_cache | 0.40.1, 0.40.0, 0.3.0, 0.11.0 | MIT OR Apache-2.0 | free |
| **stylo** | 0.24.0 (2026-11-11) | **MPL-2.0** | **ask (question 2)** |
| **stylo_dom, stylo_traits, stylo_derive, stylo_static_prefs, stylo_atoms** | 0.24.0 | **MPL-2.0** | **ask (question 2)** |
| **stylo_config** | 0.12.0 | **MPL-2.0** | **ask (question 2)** |
| **selectors** | 0.43.0 | **MPL-2.0** | **ask (question 2)** |
| **cssparser, cssparser-macros, dtoa-short** | 0.38.0, 0.7.1, 0.3.5 | **MPL-2.0** | **ask (question 2)** |
| **to_shmem, to_shmem_derive** | 0.8.0, 0.3.0 | **MPL-2.0** | **ask (question 2)** |
| **app_units, uluru** | 0.7.8, 3.1.0 | **MPL-2.0** | **ask (question 2)** |
| stylo_malloc_size_of, malloc_size_of_derive, servo_arc | 0.24.0, 0.1.3, 0.6.0 | MIT OR Apache-2.0 | free |
| euclid, smallbitvec, thin-vec, atomic_refcell, rayon | 0.22.14, 2.6.1, 0.2.21, 0.1.14, 1.12.0 | MIT OR Apache-2.0 (atomic_refcell: Apache-2.0 OR MIT) | free |
| precomputed-hash, void | 0.1.1, 1.0.2 | MIT | free |
| encoding_rs | 0.8.44 | (Apache-2.0 OR MIT) AND BSD-3-Clause | free |
| icu_segmenter | 2.4.0 | Unicode-3.0 | free (ADR 0009's free class) |
| taffy | 0.15.1 | MIT | free |
| parley, fontique | 0.13.0 | Apache-2.0 OR MIT | free |
| anyrender, anyrender_vello_cpu, anyrender_svg | 0.15.0, 0.19.0, 0.16.0 | MIT OR Apache-2.0 | free |
| vello_cpu | 0.4.0 | Apache-2.0 OR MIT | free |
| read-fonts | 0.47.0 | MIT OR Apache-2.0 | free (already in the tree through skrifa) |
| accesskit | 0.25.2 | MIT OR Apache-2.0 | free (its subset is already pinned in `uikit`) |
| wuff | 0.2.10 | MIT | free |
| keyboard-types, cursor-icon | 0.8.3, 1.2.0 | MIT OR Apache-2.0; MIT OR Apache-2.0 OR Zlib | free |

**JavaScript, the runtime, the network and media:**

| Crate or part | Version | Licence | Class |
|---|---|---|---|
| boa_engine, boa_gc, boa_parser | 0.22.0 | Unlicense OR MIT | free; the fallback |
| rquickjs, rquickjs-sys (bundles QuickJS-NG) | 0.14.0 | MIT; QuickJS-NG's repository: MIT | free; chosen by S14-3 |
| wasi-libc (linked into the renderer by wasi-sdk) | wasi-sdk 24 | MIT, Apache-2.0 and Apache-2.0 WITH LLVM-exception licence files (WebAssembly/wasi-libc); wasi-sdk's own repository: Apache-2.0 | free; wasi-sdk itself is a CI build tool and never ships |
| **mozjs**, StarlingMonkey (SpiderMonkey) | 0.27.0 | **MPL-2.0** | **ask**, only if S14-3 needs it (question 2's second part) |
| wasmtime, pulley-interpreter | 51.0.0 | Apache-2.0 WITH LLVM-exception | free (ADR 0003's second engine) |
| brotli | 9.1.3 | BSD-3-Clause AND MIT | free |
| flate2 | 1.1.11 | MIT OR Apache-2.0 | free (already in the tree) |
| h2 (B5) | 0.4.20 | MIT | free |
| tungstenite (B5) | 0.30.0 | MIT OR Apache-2.0 | free |
| psl (the crate) | 2.1.261 (2026-11-22) | MIT/Apache-2.0 | free |
| **The Public Suffix List (the data inside `psl`)** | daily | **MPL-2.0** (publicsuffix/list's licence) | **ask (question 2)** |
| png, gif, image-webp, image | 0.18.1, 0.14.2, 0.2.4, 0.25.10 | MIT OR Apache-2.0 | free |
| zune-jpeg | 0.5.15 | MIT OR Apache-2.0 OR Zlib | free |
| resvg, usvg (B4) | 0.50.0 | Apache-2.0 OR MIT | free |
| rav1d (B6) | 1.1.0 | BSD-2-Clause | free |
| libvpx (B6, C) | upstream | BSD-3-Clause (webmproject/libvpx) | free |
| libopus (B6, C) through `opus` | upstream; opus 0.4.0 | BSD-3-Clause text (xiph/opus `COPYING`); the binding MIT/Apache-2.0 | free |
| Noto fonts and Noto Color Emoji | upstream | OFL-1.1 (notofonts, googlefonts/noto-emoji) | approved for font data (ADR 0009, 2026-10-22) |

**Test suites, fetched in CI at a pinned commit and never shipped or committed:** WPT (3-Clause BSD, its `LICENSE.md`) and test262 (Ecma's BSD licence, its `LICENSE`).

**Not taken, and why it matters for the licence:**

| Part | Licence | Where it would come from |
|---|---|---|
| **GStreamer** | **LGPL-2.1** (ADR 0006's research) | Servo's media (route A). Not taken |
| **FFmpeg** (`libavcodec`) | **LGPL** (ADR 0006) | Ladybird's media (route C); inside the Linux guest only if ADR 0006 question 7 says yes |
| **EasyList** | **GPL-3.0-or-later OR CC-BY-SA-3.0** (easylist.to's licence page) | A tracker blocklist. Not taken (plan question 8) |
| servo, webrender | MPL-2.0 | Route A, re-measured at B5 |
| symphonia | MPL-2.0 | A pure-Rust audio decoder; would be an ask if track M picks it |
| woff2 | Apache-2.0 | Free, but unmaintained since 2022; wuff instead |

## Consequences

- Kernel AI gets a browser that is its own: it looks like Kai Desk, opens in under a second (S14-1's bar), costs no Linux guest's RAM, and Kai reads and drives it through typed rows rather than from outside.
- The renderer is the most attacked code in any OS. Here every site is sealed twice: by Wasm's checks and by its own process, which holds no handle but a tagged route to `webnet` and a surface.
- We own the browser layer: bindings, events, navigation, forms, storage. That is the long tail of web compatibility, and it will trail Chrome and Firefox for years on the heavy web. Track V's Firefox covers that gap, if question 1 keeps it.
- MPL-2.0 enters the shipped graph through Stylo and its 14 helper crates, and the suffix list's data, if question 2 says yes. It is the OS's own licence (ADR 0009 D1, decided 2026-10-27), so the duty is to publish our changes to those files, which we do anyway. `deny.toml` gains one exception per crate, and `NOTICES.txt` lists them.
- Blitz has had no stable release since 2025-10. We build on its 0.3 beta and may carry a fork; each monthly sync is our cost (item 11).
- The kernel gains one path, a loader for signed code images, and Wasmtime joins as ADR 0003's second engine. Both serve other apps later.
- `kai:net@0.1.0` stays small for apps. Only `webnet` holds `hosts:any`.

## What it costs to change later

| Choice | Cost to change | Why |
|---|---|---|
| Blitz's stack to Servo | Medium | Stylo, html5ever, `webnet`, the rows and the processes carry over; Taffy and Parley give way to Servo's layout, and `webjs`'s glue to SpiderMonkey's |
| The JS engine | Low to medium | `webjs` generates the glue from WebIDL; the DOM stays |
| Wasmtime with ahead-of-time code to native renderers | Medium | The renderer's code is the same Rust; it needs the `std` port and loses the Wasm wall |
| One process per site | Low | A policy in `browser` and `webnet` |
| The `web` volume and its formats | Medium | Versioned from the first release, like every on-disk format |
| The row names | Low | Provider-neutral, shared with ADR 0006 |

## Spikes that settle it

All run on the host or on GitHub runners. None runs a model, and none runs QEMU on the founder's PC.

| Spike | Build and measure | Pass | Fail means |
|---|---|---|---|
| **S14-1** `engine-in-wasm` (about 1 week) | Blitz (blitz-html, blitz-dom, anyrender_vello_cpu) built for `wasm32-wasip2`; ten fixture pages (our own, plus WPT pages) through parse, style, layout and paint, natively, on Wasmtime with Cranelift ahead of time, on Wasmtime's Pulley and on wasmi; time and peak memory on a GitHub runner | Ahead-of-time code at most 2x native; a long Wikipedia-shaped article to first paint in at most 400 ms; peak at most 256 MiB | Above 2x: option (iii), native renderers with a `std` port, costed in the spike's report. wasmi's figure is recorded for the record |
| **S14-2** `wasmtime-nostd` (about 2 weeks) | Wasmtime 51.0.0's no_std runtime with the component model in our ring 3; a `.cwasm` made in CI, signed, loaded by a throwaway kernel loader that maps it read-execute; a KVM boot case runs one component | It runs; `.text` added at most 8 MiB (est. bar); start to first call at most 50 ms under KVM | The renderer runs on wasmi or natively (S14-1 decides which) and ADR 0003's trigger waits |
| **S14-3** `js-engines` (about 1 week) | Boa 0.22.0, rquickjs 0.14.0 and a StarlingMonkey build, each for `wasm32-wasip2` on Wasmtime ahead of time: test262 run on each; Octane's Richards, DeltaBlue, RayTrace and NavierStokes; start time, memory and size | The fastest of Boa and QuickJS-ng with test262 at least 90% is taken | Both more than 3x slower than SpiderMonkey on the set: question 2's second part comes back to the founder |
| **S14-4** `web-compare` (days) | The plan's site list loaded by Firefox headless on a GitHub runner, read-only; its text in reading order and a screenshot of the first screen kept as the run's artifacts, never committed | The harness gives the same text on two runs | The site list's gate becomes text-only |

**Results** (spikes on `spike/*` branches, never merged; the plan's section 13 has the detail):
- **S14-1 passed** (#1441, run 45720633850). Blitz 0.3.0-beta.4 with Stylo 0.22 built for `wasm32-wasip2` with no patches. Ahead-of-time code is 1.62x native, first paint 107 ms, peak 30 MiB. Without `simd128` it is 2.50x; wasmi is 36.7x and Pulley 57x. Option (ii) stands.
- **S14-2 is partly done** (#1438). Wasmtime 51's runtime builds for our ring 3 with about 60 lines of glue. A signed `.cwasm` runs natively from read-execute pages and on Pulley, and a flipped bit is refused. target-lexicon cannot parse `x86_64-kai-user`, so the user target is renamed once (plan task F-0). The component model and the figures against the bars remain.
- **S14-3 is done** (run 45720308618). QuickJS-NG is taken (item 3).
- **S14-4 is done** (run 45732633843). 10 of 12 sites gave the same text twice; two news pages change live, and two sites gave Firefox no text with cookies refused. The site gates therefore compare against Firefox in the same job, and those two sites are tracked, not gated.

## Phases that depend on it

| Phase | What it needs from this ADR |
|---|---|
| P5 | Nothing more: `webnet` builds on `kainet` as it is |
| P6 | Track B's milestones B0-B4 for v0.2 (question 6), B5-B6 after |
| P7 | Real hardware's audio for B6's sound on the founder's PC |
| P8 | A security review of `webview`, `webnet` and the code loader; fuzzing at scale; out-of-process frames if not done in B5 |

## Changes to recently merged and open work

- **ADR 0006 (accepted).** On approval, its Status line notes "partly superseded by ADR 0014: the `browser_*` rows are answered by Kai Browser first; the VM is P6's track V". Nothing else in 0006 changes. Its item 7 rows keep their names, and the VM's Firefox registers as a second provider, chosen by the person or by `browser_open`'s `engine` argument. 0006's rows take a `window`; Kai Browser's take a `tab`, and a VM window counts as one tab.
- **ADR 0003 (accepted).** Its second-engine triggers gain one: "the web engine's renderers". No text changes until Wasmtime lands (plan task F-1).
- **`kai:net@0.1.0`.** No change.
- **`nethosts` and the shell's ceiling.** No change. `webnet`'s ceiling is its own plan line, `netdest=proto:tcp,ports:443+80,hosts:any` (question 3).
- **The ADR index** gains this row, marked proposed.

## Open questions for the founder

These are questions 1-6 of the P6 plan's section 5, which says what each answer changes. Questions 7-9 there (the search default, privacy defaults, H.264) are product choices inside this design and change none of its items.
1. Our own browser first, with Firefox in the Linux VM kept as the second route. Default: yes.
2. The MPL-2.0 crates: Stylo and its parts, selectors, cssparser and their small helpers (the 15 crates marked "ask" above), and the Public Suffix List's data. SpiderMonkey is asked again only if S14-3 needs it. Default: approve the first group as one row.
3. The browser reaches any website without asking site by site. Default: yes.
4. TLS 1.2 for the browser alone. Default: yes.
5. Engine code compiled ahead of time in CI and signed. Default: yes.
6. v0.2 ships the browser at milestone B4, and the VM's YouTube test moves after it. Default: yes.

## Sources

External, read 2026-11-23:
- crates.io API (versions, licences, dates, and the dependency lists of `stylo` 0.24.0, `blitz-dom` 0.2.4 and `blitz-dom` 0.3.0-beta.4): every crate in "Crates and licences", plus servo 0.8.0 and webrender 0.70.1 (MPL-2.0), symphonia 0.6.1 (MPL-2.0) and woff2 0.3.0 (Apache-2.0, 2022-05-05)
- GitHub API, repository licence files: webmproject/libvpx (BSD-3-Clause), xiph/opus `COPYING` (BSD-3-Clause text), quickjs-ng/quickjs (MIT), publicsuffix/list (MPL-2.0), web-platform-tests/wpt `LICENSE.md` (3-Clause BSD), tc39/test262 `LICENSE` (BSD), notofonts/noto-cjk `Sans/LICENSE` and googlefonts/noto-emoji (OFL-1.1); DelSkayn/rquickjs README (binds QuickJS-NG)
- EasyList's licence page: https://easylist.to/pages/licence.html
- Blitz README and crate pages: https://github.com/DioxusLabs/blitz, https://docs.rs/crate/blitz/0.2.1
- Boa v0.22 release notes (mirror): https://newreleases.io/project/github/boa-dev/boa/release/v0.22; v0.21 post: https://boajs.dev/blog/2025/10/22/boa-release-21
- StarlingMonkey: https://github.com/bytecodealliance/StarlingMonkey; ComponentizeJS: https://github.com/bytecodealliance/componentizejs; weval: https://github.com/tekknolagi/weval and https://arxiv.org/abs/2411.10559
- Servo releases (Phoronix on 0.5 and 0.7): https://phoronix.com/news/Servo-0.7-Changes; servo.org: https://servo.org/about/
- Ladybird, litehtml and V8 facts are ADR 0006's (read 2026-10-21) or general and marked unverified above

Repository (main at 682411c5): docs/decisions.md:278 (the OS is MPL-2.0 with the DCO, decided 2026-10-27); deny.toml:32-43 (today's licence exceptions); cpufeat/src/lib.rs:106-107; wit/kai-net/http.wit:26, :66; kabi/src/plan.rs:30; gfx2d/Cargo.toml; assets/fonts; uipaint/src/lib.rs:8-9; uikit/src/a11y.rs:1-3; kernel/src/process/mod.rs:58; user/x86_64-kai-user.json; agentabi/src/net.rs:116; docs/adr/0003-app-model.md §1; docs/adr/0006-existing-software.md items 1-13; docs/plans/p5-networking.md sections 2 and 11.
