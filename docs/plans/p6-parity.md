# P6 plan: everyday-OS parity, with the browser first

Status: **DRAFT for the founder's approval**, proposed 2026-11-23. Nothing lands as code until the founder approves it. The browser engine is decided in [ADR 0014](../adr/0014-browser-engine.md), proposed with this plan. Sections 5-12 are still to be written (see "Still to write" at the end).

The founder asked on 2026-11-23: "I want to do the browser project, I want this to be like any other operating system with all features no matter how large of a roadmap or how long itll take." This plan turns that into tracks. P6 changes from "running existing apps" (roadmap, P6 row) to **everyday-OS parity**. ADR 0006's Linux VM, which was all of P6, becomes one track in it (track V).

Repo facts are on main at 682411c5. Crate versions and licences were read from the crates.io API on 2026-11-23. Sizes marked "est." are estimates. Nothing was built or run for this plan, and no model or QEMU ran.

## 1. Goal and exit

**Goal:** a person who uses Windows 11, macOS, ChromeOS or Ubuntu finds the basic things they use every day, with Kai built in. The browser comes first and in the most detail.

**What the founder sees at v0.2's part of P6** (question 6), in a VM:
- **A browser of our own.** Browser opens Wikipedia, MDN, gov.uk, Hacker News, DuckDuckGo's results, BBC News and the rest of section 4.4's tier-1 list, with images, links, forms and then JavaScript. Each site runs sealed in its own process.
- **Kai uses it safely.** "open wikipedia" runs; "what does this page say about prices" reads the page as data; filling in or sending a form asks first; a page that tells Kai to send your notes somewhere starts nothing.
- **Copy and paste works** between every app, and screenshots and notifications exist.
- **Pictures, PDFs and sound** open and play.

**Exit for v0.2 (est. list; boot cases under KVM and TCG with the HTTPS mock):** `web-hello`, `browser-static`, `browser-forms`, `browser-download`, `browser-cookies`, `browser-js`, `browser-kai`, `browser-pf`, `desk-clipboard`, `desk-notify`, `desk-screenshot`, `app-photos`, `app-pdf`, `audio-out`. Host gates: section 4.3's WPT sets and the site list on a GitHub runner against Firefox headless. Real-model gates on GitHub only: fluency at least 47/50, actbench with browser tasks, and S02-1's bait probes at 0 false runs.

## 2. Where Kernel AI stands: the gap list

"Today" was read from the code on main at 682411c5. The other four systems are named by what they ship.

| # | Feature | Windows 11 / macOS / ChromeOS / Ubuntu | Kernel AI today | Track |
|---|---|---|---|---|
| 1 | Web browser | Edge / Safari / Chrome / Firefox | None. ADR 0006's Firefox-in-VM has only `cpufeat`'s SVM bits (cpufeat/src/lib.rs:106-107); Kai's `net_fetch` reads a page as text | B |
| 2 | Copy and paste between apps | all | None: no clipboard anywhere in the code | D |
| 3 | Viewing pictures | Photos / Preview / Gallery / Image Viewer | None: `uikit` draws an image as a box (uipaint/src/lib.rs:8-9); no PNG or JPEG decoder | M |
| 4 | PDFs | Edge / Preview / Gallery / Papers | None | M |
| 5 | Sound | all | None: no audio driver; ADR 0007's `audiod` is not built | M, H |
| 6 | Music and video | Media Player / Music, QuickTime / Gallery / Videos | None | M |
| 7 | Runs on my own PC, with an installer | all | Runs in QEMU and VirtualBox; NVMe, xHCI keyboards and mice, virtio; no installer; P7 not started | H (P7) |
| 8 | Wi-Fi | all | None: wired virtio-net only | H |
| 9 | Bluetooth | all | None | H |
| 10 | USB sticks and drives | all | None: xHCI drives keyboards, mice and tablets only (xhci/src/lib.rs:1-2) | H |
| 11 | Power off, sleep, battery | all | Restart only; power off needs ACPI's \_S5 (kernel/src/process/syscall/power.rs:14); no sleep | H |
| 12 | Several displays | all | One screen (`deskgeom::Layout::for_screen`) | H, D |
| 13 | GPU acceleration | DirectX / Metal / yes / Mesa | CPU drawing (`gfx2d` on tiny-skia) to virtio-gpu 2D or GOP | H (later) |
| 14 | Notifications | Action Center / Notification Center / yes / GNOME | None: only the Kai chip on the taskbar | D |
| 15 | Screenshots | Snipping Tool / Cmd-Shift-4 / yes / yes | None for people (xtask's tour shots are tests) | D |
| 16 | Drag and drop; "Open with" | all | None: each app opens its own files | D |
| 17 | Mail | Outlook / Mail / Gmail / Thunderbird | None | W |
| 18 | Calendar and contacts | all | None | W |
| 19 | Office documents | Office / Pages, Numbers / Google Docs / LibreOffice | Notes and the editor; no .docx, .xlsx or .odt | W |
| 20 | Calculator, clock, alarms | all | None | W |
| 21 | Zip files | all | None | M |
| 22 | Printing | all | None | W (late) |
| 23 | Lock screen and sign-in | all | None: boots to the desktop | U |
| 24 | Several people, accounts | all | One person, `/home/user`; accounts are P8 | U (P8) |
| 25 | Disk encryption | BitLocker / FileVault / yes / LUKS | None (P8) | U (P8) |
| 26 | Backup to another disk | File History / Time Machine / cloud / Déjà Dup | kaifs snapshots and `undo` on the same disk only | U |
| 27 | Screen reader, magnifier | Narrator / VoiceOver / ChromeVox / Orca | An accessibility tree for every window (uikit/src/a11y.rs:1-3) that Kai reads; a high-contrast theme; nothing speaks or magnifies | A |
| 28 | Keyboard layouts, input methods | all | US layout only (deskcore/src/keymap.rs:2); no input method | L |
| 29 | Languages and scripts | all | English only; Inter and JetBrains Mono; harfrust shapes text, but no CJK, Arabic or Indic fonts and no right-to-left layout | L |
| 30 | Time zones with daylight saving | all | UTC or a fixed offset (ADR 0005, built 2026-11-18) | L |
| 31 | App store with others' apps | Store / App Store / Play / Snap Store | Start's catalogue and the signed online repository (P5); Kernel AI's own apps only until v1.0 (ADR 0011 question 5) | S |
| 32 | Existing apps | Win32, WSL / Mac apps / Android, Linux / Linux | WebAssembly apps; ADR 0006's Linux VM not started | V |
| 33 | Camera and microphone | all | None | H (late) |

**Where Kernel AI is already level or ahead:** signed A/B updates with rollback (ADR 0008); `undo` for file actions and installs; per-app network grants that the person sees and revokes (P5); local search over files (ADR 0013); Settings, Files, Terminal, Editor, System Monitor and Models; and Kai, a local assistant that acts only through one consent door.

## 3. The tracks, ranked

Ranked by how much a new user would miss each one. Tier 1 is "leaves in the first hour without it".

| Rank | Track | Closes rows | Tier | Where it lands |
|---|---|---|---|---|
| 1 | **B, the browser** | 1 | 1 | P6; v0.2 at milestone B4 (question 6) |
| 2 | **D, desktop basics** | 2, 14-16 | 1-2 | P6, v0.2 (clipboard first) |
| 3 | **M, media and documents** | 3-6, 21 | 1 | P6: pictures, PDF and sound in v0.2; music and video after |
| 4 | **H, hardware and install** | 7-13, 33 | 1 | P7 (v0.3) and on; ADR 0007 |
| 5 | **W, work apps** | 17-20, 22 | 2 | Late P6 and P7 |
| 6 | **L, languages and input** | 28-30 | 2-3 | From P6; input methods by P8 |
| 7 | **U, people and safety** | 23-26 | 2-3 | Lock screen and backup in P6-P7; accounts and encryption in P8 |
| 8 | **A, accessibility** | 27 | 3 by count, but essential to those who need it | Keyboard-only and text size from P6; the screen reader by v1.0 |
| 9 | **S and V, apps from others** | 31-32 | 3 | V (ADR 0006's VM) after B1 if question 1 says yes; others' apps after v1.0 |

**F, foundations**, is not ranked because a person never sees it: Wasmtime as ADR 0003's second engine, a loader for signed code images, a WASI 0.2 subset host, `kainet`'s request bodies and headers, child surfaces, and the Public Suffix List.

## 4. Track B, the browser

ADR 0014 has the design. In short: Blitz's parts (html5ever, Stylo, Taffy, Parley, vello_cpu) with our own browser layer; JavaScript by Boa (or QuickJS-ng after spike S14-3); one `webview` process per site, a Wasm component on Wasmtime with code compiled ahead of time in CI; `webnet`, a native service on `kainet`, holds the network, cookies and cache; the `browser` app holds the tabs and the address bar.

### 4.1 Tasks

Each row is one PR unless it says otherwise. Lanes: **W-e** (engine: `webcore/`, `webjs/`, `apps/webview/`), **W-n** (`webnet/`, `user/src/bin/webnet/`), **W-a** (`apps/browser*`), all new.

| # | Task | Lane | Needs | Size (est.) | Exit check |
|---|---|---|---|---|---|
| B-1 | `webcore`: Blitz's DOM, style, layout and paint wrapped, a page to pixels on the host; golden images | W-e | S14-1 | L | Host goldens |
| B-2 | WPT reftests on the host: `cargo xtask wpt` at a pinned WPT commit fetched in CI, never committed; `web-wpt.yml`; the baseline recorded | W-e | B-1 | M | Baseline row |
| B-3 | `webnet` logic: HTTP over `kainet` with bodies, headers, streaming, gzip and brotli, keep-alive; partitioned cookies; cache; HSTS; CORS and opaque-response checks; hostile tests and fuzz targets | W-n | F-4 | L | Host and hostile tests |
| B-4 | `webnet` in the OS: its plan line, the `web` volume, the site-tagged route, downloads into `/home/user/Downloads` | W-n, K's and S-io's turns | B-3 | M | `browser-download` |
| B-5 | `webview`: `webcore` on Wasmtime, a child surface, input, scrolling | W-e | F-1, F-2, F-3, F-5 | M | `web-hello` |
| B-6 | The `browser` app: tabs, address bar, back, forward, reload, history, bookmarks, find, zoom, downloads | W-a | B-5 | L | `browser-static` |
| B-7 | Images in pages through `kaimedia` (M-1) | W-e | M-1 | S | WPT images subset |
| B-8 | Fonts: Noto families, emoji, fallback; WOFF and WOFF2 | W-e, model lane's turn | B-1 | M | Host goldens per script |
| B-9 | Forms without JavaScript: every input type, GET and POST, HTTPS first | W-e, W-n | B-4, B-6 | M | `browser-forms`, `browser-cookies` |
| B-10 | Kai's browser rows (ADR 0014 item 10) | A-core, model lane | B-6 | M | `browser-kai`, `browser-pf`; fluency, actbench, bait probes |
| B-11 | `webjs`: bindings generated from WebIDL; the JS engine in the renderer; event loop, timers, console | W-e | S14-3, B-5 | L | test262 at least 90% |
| B-12 | DOM core for scripts; testharness.js runs | W-e | B-11 | L | `browser-js`; WPT `dom/` |
| B-13 | fetch, XHR, URL, history API, web storage | W-e, W-n | B-12 | L | WPT `fetch`, `xhr`, `webstorage` |
| B-14 | Canvas 2D, SVG (resvg), CSS animations, observers | W-e | B-12 | L | WPT subsets |
| B-15 | The site-list job: `web-sites.yml` on a GitHub runner against Firefox headless (S14-4's harness) | W-e | S14-4 | S | Its own run |
| B-16 | Milestone B5: workers, IndexedDB, WebSocket, HTTP/2, out-of-process frames | W-e, W-n | B-13 | several PRs | WPT subsets; tier 3a |
| B-17 | Milestone B6: audio and video, WebM, VP9, AV1, Opus, MSE | W-e, MD | M-4, M-6 | several PRs | `browser-media` |
| B-18 | The browser's v0.2 close: cases in `ci.yml`, decisions rows, roadmap status | – | B4 | S | Section 1's exit |

### 4.2 Milestones and gates

The pass-rate bars are first estimates, set before B-2's baseline exists. B-2's PR records the baseline and may move a bar, with the manager's OK and a decisions row.

| Milestone | Tasks | Gate |
|---|---|---|
| **B0** the engine in the OS | B-1, B-2, B-3, B-5; F-1 to F-5 | S14-1 to S14-3 pass; `web-hello` renders a fixture under KVM and TCG |
| **B1** static pages | B-4, B-6, B-7, B-8 | WPT set R1 at least 70% on the host; tier-1 sites' text at least 95% of Firefox's words, with no crash or hang; first paint of the Wikipedia fixture at most 1 s under KVM |
| **B2** forms and Kai | B-9, B-10 | Its cases; fluency at least 47/50; bait probes 0 false runs |
| **B3** first JavaScript | B-11, B-12 | test262 at least 90%; WPT `dom/nodes` and `dom/events` at least 60%; tier-2a sites |
| **B4** the interactive web (v0.2) | B-13, B-14, B-15, B-18 | WPT `fetch/api/basic` 60%, `xhr` 50%, `webstorage` 80%, R1 80%; all tier-2 sites |
| **B5** web apps | B-16 | WPT `websockets` 60%, `IndexedDB` 50%; tier 3a |
| **B6** media | B-17 | A self-hosted MSE page with VP9 and Opus plays 60 s at 720p30 with at most 1% dropped frames under KVM, sound for at least 55 s (S06-2's bars); youtube.com by hand |

### 4.3 WPT sets

R1 (reftests): `css/CSS2/normal-flow`, `floats`, `margin-padding-clear`, `box-display`, `linebox`; `css/css-flexbox`; `css/css-text/white-space`; `css/css-backgrounds`; `css/css-fonts` (weight, style, size); `css/selectors`; `html/rendering/non-replaced-elements`. `css/css-grid` joins at B4.

### 4.4 Sites that must work

Loaded live on a GitHub runner only, never committed (section 8 item 4, to write). The founder or the manager may add a site by a one-line PR.
- **Tier 1 (B1, no JavaScript needed):** en.wikipedia.org, developer.mozilla.org, www.gov.uk, news.ycombinator.com, html.duckduckgo.com, text.npr.org, lite.cnn.com, docs.rs, github.com (a README, signed out), stackoverflow.com, www.bbc.co.uk/news, forecast.weather.gov.
- **Tier 2 (B3-B4):** duckduckgo.com, github.com (issues and file view), crates.io, docs.python.org, www.rust-lang.org, www.mozilla.org, users.rust-lang.org, www.theguardian.com, archive.org, lobste.rs.
- **Tier 3a (B5):** www.openstreetmap.org, excalidraw.com.
- **Tier 3 (track V's Firefox for now):** youtube.com, docs.google.com, mail.google.com, figma.com. Netflix and other Widevine sites cannot work in any browser of ours (section 12, to write).

## 5. Entry, and the founder's answers P6 needs

P6 work lands on main; v0.1 fixes stay on `release/v0.1`, as for P5. Each row says what this plan builds until you answer, and what a "no" changes.

| Question | Default this plan builds | If you say no |
|---|---|---|
| **1. Our own browser first, with Firefox in the Linux VM (ADR 0006) kept as the second route** for heavy sites and Linux apps | Yes. Browser is "the browser"; track V starts when a lane is free after B1 | VM only: no page shows until the hypervisor works (7-15 PM, ADR 0006). Native only: YouTube and web office suites wait years |
| **2. MPL-2.0 crates**: Stylo, selectors and cssparser (Servo's CSS parts), and the Public Suffix List's data (`psl`). They are not on the no-approval list. MPL-2.0 is the OS's own licence, so they fit; the duty is to publish changes to those files | Approve them as one row. SpiderMonkey (MPL-2.0) is asked again only if S14-3 needs it | We write our own CSS engine (est. +12-18 PM, and much weaker CSS), and cookies cannot be kept apart per site correctly without the suffix list |
| **3. The browser reaches any website** without a card per site (`hosts:any` on ports 443 and 80, for `webnet` only) | Yes, as every browser does; plain `http://` only after a click-through page | A card for every new site a page touches, which no browser does |
| **4. TLS 1.2 for the browser alone** (P5 is TLS 1.3 only) | Yes, through rustls's `tls12` switch in `webnet` only | Sites with only TLS 1.2 fail with a named error |
| **5. Engine code compiled ahead of time in CI and signed** (ADR 0003 already allows it for first-party packages; this turns it on) | Yes | The renderer runs on wasmi (S14-1 says how slow) or natively, with a weaker wall around hostile pages |
| **6. v0.2 ships the browser at B4**, with clipboard, screenshots, notifications, pictures, PDF and sound; the VM's YouTube test moves after it | Yes | v0.2 waits for ADR 0006's YouTube exit |
| 7. The search box's default | DuckDuckGo (its HTML page works before JavaScript); you can change it; no deals | You name another |
| 8. Privacy defaults | Cookies kept apart per site; no sync, no telemetry; history kept 90 days on the PC, with `undo`. No tracker blocklist, because EasyList is GPL-3.0 or CC-BY-SA-3.0 | Approve EasyList's licence and we add list blocking |
| 9. H.264 and AAC video (ADR 0006 question 7) | Still off: VP9, AV1 and Opus only, asked again at B6 | On: patent and licence questions come with it |

## Still to write

The PC shut down before these were written; the Mac session finishes them from ADR 0014 and this outline:
- **6. The other tracks**: task tables with gates for F (F-1 Wasmtime backend, F-2 signed code images, F-3 WASI 0.2 subset, F-4 `kainet` bodies, headers and TLS 1.2, F-5 child surfaces, F-6 the suffix list), D (D-1 clipboard, D-2 notifications, D-3 screenshots, D-4 open with, D-5 drag and drop, D-6 snapping), M (M-1 `kaimedia`, M-2 Photos, M-3 PDF on hayro 0.9.0, M-4 `audiod`, M-5 music, M-6 video, M-7 zip), H (P7's list), W, L, A, U, S and V.
- **7. Sessions and hot-file turns**: two sessions at approval (spikes S14-1, S14-3, S14-4 then B-1 and B-2; S14-2 then F-1 to F-3 as lane C's turn); D-1 as G-core's next item.
- **8. Decided in this plan**, **9. Spikes** (ADR 0014's S14-1 to S14-4), **10. Tests**, **11. Crates and licences** (the crates.io reads are in ADR 0014's sources), **12. Estimate, risks and out of scope** (no JIT, WebGL, DRM, extensions, sync, WebRTC before their tracks).
