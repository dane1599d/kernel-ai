# P6 plan: everyday-OS parity, with the browser first

Status: **proposed for the founder's approval**, 2026-11-23. Nothing lands as code until the founder approves it. The browser engine is decided in [ADR 0014](../adr/0014-browser-engine.md), proposed with this plan.

The founder asked on 2026-11-23: "I want to do the browser project, I want this to be like any other operating system with all features no matter how large of a roadmap or how long itll take." This plan turns that into tracks. P6 changes from "running existing apps" (roadmap, P6 row) to **everyday-OS parity**. ADR 0006's Linux VM, which was all of P6, becomes one track in it (track V).

Repo facts were read on main at 682411c5 and rechecked on main at 42869af8 (2026-11-26), after the groundwork in section 13. Crate versions and licences were read from the crates.io API on 2026-11-23, and the licences of non-crate parts from their repositories through the GitHub API the same day (section 11). Sizes marked "est." are estimates. Nothing was built or run for this plan, and no model or QEMU ran.

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
| 1 | Web browser | Edge / Safari / Chrome / Firefox | None. ADR 0006's Firefox-in-VM has `cpufeat`'s SVM bits (cpufeat/src/lib.rs:106-107) and the host-tested `svm` crate (#1449), but no run loop or `vmm`; Kai's `net_fetch` reads a page as text | B |
| 2 | Copy and paste between apps | all | None: no clipboard anywhere in the code | D |
| 3 | Viewing pictures | Photos / Preview / Gallery / Image Viewer | None: `uikit` draws an image as a box (uipaint/src/lib.rs:8-9); no PNG or JPEG decoder | M |
| 4 | PDFs | Edge / Preview / Gallery / Papers | None | M |
| 5 | Sound | all | None: no audio driver; ADR 0007's `audiod` is not built | M, H |
| 6 | Music and video | Media Player / Music, QuickTime / Gallery / Videos | None | M |
| 7 | Runs on my own PC, with an installer | all | Runs in QEMU and VirtualBox; NVMe, xHCI keyboards and mice, virtio; no installer; P7 not started | H (P7) |
| 8 | Wi-Fi | all | None: wired virtio-net only | H |
| 9 | Bluetooth | all | None | H |
| 10 | USB sticks and drives | all | None: xHCI drives keyboards, mice and tablets only (xhci/src/lib.rs:1-2) | H |
| 11 | Power off, sleep, battery | all | Restart, and power off through the firmware's ResetSystem (POWER_OFF, call 134, since 2026-11-18); ACPI's \_S5 through uACPI comes later (kernel/src/process/syscall/power.rs:17); no sleep, no battery | H |
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

**F, foundations**, is not ranked because a person never sees it: Wasmtime as ADR 0003's second engine, a loader for signed code images, a WASI 0.2 subset host (today's `wasihost` serves only the twenty WASI preview-1 calls xcc needs, for `cc`), `kainet`'s request bodies and headers, child surfaces, and the Public Suffix List. Section 6.1 has its tasks.

## 4. Track B, the browser

ADR 0014 has the design. In short: Blitz's parts (html5ever, Stylo, Taffy, Parley, vello_cpu) with our own browser layer; JavaScript by QuickJS-NG, which spike S14-3 picked (Boa is the fallback); one `webview` process per site, a Wasm component on Wasmtime with code compiled ahead of time in CI; `webnet`, a native service on `kainet`, holds the network, cookies and cache; the `browser` app holds the tabs and the address bar.

### 4.1 Tasks

Each row is one PR unless it says otherwise. Lanes: **W-e** (engine: `webcore/`, `webjs/`, `apps/webview/`), **W-n** (`webnet/`, `user/src/bin/webnet/`), **W-a** (`apps/browser*`), all new.

| # | Task | Lane | Needs | Size (est.) | Exit check |
|---|---|---|---|---|---|
| B-1 | `webcore`: Blitz's DOM, style, layout and paint wrapped, a page to pixels on the host; golden images. Starts from Blitz 0.3.0-beta.4 and moves it to Stylo 0.24 and Parley 0.13 (ADR 0014, option B), offered upstream; the MPL-2.0 crates enter `deny.toml` here, on question 2 | W-e | S14-1; question 2 | L | Host goldens; `cargo deny` clean |
| B-2 | WPT reftests on the host: `cargo xtask wpt` at a pinned WPT commit fetched in CI, never committed; `web-wpt.yml`; the baseline recorded | W-e | B-1 | M | Baseline row |
| B-3 | `webnet` logic: HTTP over `kainet` with bodies, headers, streaming, gzip and brotli, keep-alive; partitioned cookies; cache; HSTS; CORS and opaque-response checks; hostile tests and fuzz targets | W-n | F-4 | L | Host and hostile tests |
| B-4 | `webnet` in the OS: its plan line, the `web` volume, the site-tagged route, downloads into `/home/user/Downloads` | W-n, K's and S-io's turns | B-3 | M | `browser-download` |
| B-5 | `webview`: `webcore` on Wasmtime, a child surface, input, scrolling | W-e | F-1, F-2, F-3, F-5 | M | `web-hello` |
| B-6 | The `browser` app: tabs, address bar, back, forward, reload, history, bookmarks, find, zoom, downloads | W-a | B-5 | L | `browser-static` |
| B-7 | Images in pages through `kaimedia` (M-1) | W-e | M-1 | S | WPT images subset |
| B-8 | Fonts: Noto families, emoji, fallback; WOFF and WOFF2 through wuff (ADR 0014 item 7). Shared with track L's L-3 | W-e, G-core's turn | B-1 | M | Host goldens per script |
| B-9 | Forms without JavaScript: every input type, GET and POST, HTTPS first | W-e, W-n | B-4, B-6 | M | `browser-forms`, `browser-cookies` |
| B-10 | Kai's browser rows (ADR 0014 item 10) | A-core, model lane | B-6 | M | `browser-kai`, `browser-pf`; fluency, actbench, bait probes |
| B-11 | `webjs`: bindings generated from WebIDL; QuickJS-NG (rquickjs) in the renderer, its C built by wasi-sdk in CI; event loop, timers, console | W-e | B-5 | L | test262 at least 90% without `built-ins/Temporal`; Temporal by B4 |
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
| **B1** static pages | B-4, B-6, B-7, B-8 | WPT set R1 at least 70% on the host; each gated tier-1 site's text at least 95% of the words Firefox read in the same job (S14-4: live sites change between runs), with no crash or hang; first paint of the Wikipedia fixture at most 1 s under KVM |
| **B2** forms and Kai | B-9, B-10 | Its cases; fluency at least 47/50; bait probes 0 false runs |
| **B3** first JavaScript | B-11, B-12 | test262 at least 90%; WPT `dom/nodes` and `dom/events` at least 60%; tier-2a sites |
| **B4** the interactive web (v0.2) | B-13, B-14, B-15, B-18 | WPT `fetch/api/basic` 60%, `xhr` 50%, `webstorage` 80%, R1 80%; all tier-2 sites |
| **B5** web apps | B-16 | WPT `websockets` 60%, `IndexedDB` 50%; tier 3a |
| **B6** media | B-17 | A self-hosted MSE page with VP9 and Opus plays 60 s at 720p30 with at most 1% dropped frames under KVM, sound for at least 55 s (S06-2's bars); youtube.com by hand |

### 4.3 WPT sets

R1 (reftests): `css/CSS2/normal-flow`, `floats`, `margin-padding-clear`, `box-display`, `linebox`; `css/css-flexbox`; `css/css-text/white-space`; `css/css-backgrounds`; `css/css-fonts` (weight, style, size); `css/selectors`; `html/rendering/non-replaced-elements`. `css/css-grid` joins at B4.

### 4.4 Sites that must work

Loaded live on a GitHub runner only, never committed (section 8, item 4). The founder or the manager may add a site by a one-line PR.
- **Tier 1 (B1, no JavaScript needed):** en.wikipedia.org, developer.mozilla.org, www.gov.uk, news.ycombinator.com, html.duckduckgo.com, text.npr.org, lite.cnn.com, docs.rs, github.com (a README, signed out), forecast.weather.gov.
- **Tracked, not gated:** stackoverflow.com and www.bbc.co.uk/news. With every cookie refused, Firefox itself read 0 and 17 words from them in S14-4 (a consent or bot wall), so they give no reference. They become gated once the harness passes their consent page without accepting tracking.
- **Tier 2 (B3-B4):** duckduckgo.com, github.com (issues and file view), crates.io, docs.python.org, www.rust-lang.org, www.mozilla.org, users.rust-lang.org, www.theguardian.com, archive.org, lobste.rs.
- **Tier 3a (B5):** www.openstreetmap.org, excalidraw.com.
- **Tier 3 (track V's Firefox for now):** youtube.com, docs.google.com, mail.google.com, figma.com. Netflix and other Widevine sites cannot work in any browser of ours (section 12.3).

## 5. Entry, and the founder's answers P6 needs

P6 work lands on main; v0.1 fixes stay on `release/v0.1`, as for P5. Entry needs nothing more: P5 is closed but for `Hold`, and the spikes and host tasks start at approval. M10b (demand paging), which the roadmap puts before P6, limits only how many renderers run at once (section 12.2). Each row says what this plan builds until you answer, and what a "no" changes.

| Question | Default this plan builds | If you say no |
|---|---|---|
| **1. Our own browser first, with Firefox in the Linux VM (ADR 0006) kept as the second route** for heavy sites and Linux apps | Yes. Browser is "the browser"; track V starts when a lane is free after B1 | VM only: no page shows until the hypervisor works (7-15 PM, ADR 0006). Native only: YouTube and web office suites wait years |
| **2. MPL-2.0 crates**: Stylo and its parts, selectors and cssparser (Servo's CSS parts) and their small helpers, 15 crates in all (section 11), and the Public Suffix List's data inside `psl`. They are not on the no-approval list. MPL-2.0 is the OS's own licence (decided 2026-10-27), so they fit; the duty is to publish changes to those files | Approve them as one row. SpiderMonkey (MPL-2.0) is asked again only if S14-3 needs it | We write our own CSS engine (est. +12-18 PM, and much weaker CSS), and cookies cannot be kept apart per site correctly without the suffix list |
| **3. The browser reaches any website** without a card per site (`hosts:any` on ports 443 and 80, for `webnet` only) | Yes, as every browser does; plain `http://` only after a click-through page | A card for every new site a page touches, which no browser does |
| **4. TLS 1.2 for the browser alone** (P5 is TLS 1.3 only) | Yes, through rustls's `tls12` switch in `webnet` only | Sites with only TLS 1.2 fail with a named error |
| **5. Engine code compiled ahead of time in CI and signed** (ADR 0003 already allows it for first-party packages; this turns it on) | Yes | The renderer runs on wasmi (S14-1 says how slow) or natively, with a weaker wall around hostile pages |
| **6. v0.2 ships the browser at B4**, with clipboard, screenshots, notifications, pictures, PDF and sound; the VM's YouTube test moves after it | Yes | v0.2 waits for ADR 0006's YouTube exit |
| 7. The search box's default | DuckDuckGo (its HTML page works before JavaScript); you can change it; no deals | You name another |
| 8. Privacy defaults | Cookies kept apart per site; no sync, no telemetry; history kept 90 days on the PC, with `undo`. No tracker blocklist, because EasyList is GPL-3.0 or CC-BY-SA-3.0 | Approve EasyList's licence and we add list blocking |
| 9. H.264 and AAC video (ADR 0006 question 7) | Still off: VP9, AV1 and Opus only, asked again at B6 | On: patent and licence questions come with it |

## 6. The other tracks

Each row is one PR unless it says otherwise. Sizes are S (at most 2 weeks), M (2-4 weeks) and L (1-2 months) for one engineer (est.), as in ADR 0006. Lanes: **W-e**, **W-n** and **W-a** as in section 4.1; **MD**, media and documents (new: `kaimedia/`, `apps/photos*`, `apps/pdf*`, `apps/player*`); and the running lanes **K**, **C**, **G-core**, **A-core**, **AP-2**, **S-io**, **N-t** (`kainet/`) and the model lane. Within each track the rows are ranked: the first is the one a person misses most.

### 6.1 Track F, foundations (milestone B0)

| # | Task | Lane | Needs | Size | Exit check |
|---|---|---|---|---|---|
| F-0 | **The user target's name.** target-lexicon, which Wasmtime's build reads, cannot parse `x86_64-kai-user` (S14-2); pick one name it parses (S14-2 used `x86_64-kai-none`) and move `user/`, xtask and `ci.yml` to it once | K, L's review | – | S | Every ring-3 program builds; no boot case changes |
| F-1 | **Wasmtime as ADR 0003's second engine.** wasmtime 51.0.0's no_std runtime with the component model in ring 3, its platform layer (about 60 lines in S14-2), and a host that runs a component from a `.cwasm`. Built with `simd128`, signal-free traps, and the stack settings S14-1 found (an 8 MiB guest stack, `max_wasm_stack` to match, and a nesting cap for hostile pages) | C, K's review | S14-2, F-0 | M | Boot case `wasmtime-hello` under KVM and TCG |
| F-2 | **Signed code images.** The kernel maps a `.cwasm`'s code read-execute only after checking its signature against the package's (ADR 0011); never writable and executable at once | K | S14-2 | M | `code-image`: a flipped byte or a wrong key is refused |
| F-3 | **A WASI 0.2 subset host** for `wasm32-wasip2` components: clocks, random, standard output to the log; no files and no sockets (the renderer's only outside world is `webnet` and its surface). Beside `wasihost`, which stays xcc's preview-1 host | C | F-1 | M | Host tests; `web-hello` |
| F-4 | **`kainet` for the browser.** Request methods with bodies, request and response headers, bodies streamed past today's 64 KiB, gzip and brotli, keep-alive, and TLS 1.2 behind rustls's `tls12` switch, built into `webnet` only (question 4). `kai:net@0.1.0` for apps does not change | N-t | – | M | Host and hostile tests; `http_response` fuzz target grows |
| F-5 | **Child surfaces** in `kai-surface 1` (ADR 0014 item 4): a rectangle of a parent window that another process draws, clipped by `desktop`, its pixels carried on a KRNG v1 ring (`kabi/src/ring.rs`, kind `MSG`); its accessibility subtree joins the parent's, with the `data` provenance | G-core, K's turn | – | M | `desk-child-surface`: a child's pixels never reach the parent's title bar |
| F-6 | **The Public Suffix List** through `psl`, its data refreshed by the browser package's signed updates | W-n | question 2 | S | The list's own test vectors on the host |

### 6.2 Track D, desktop basics (v0.2)

| # | Task | Lane | Needs | Size | Exit check |
|---|---|---|---|---|---|
| D-1 | **Copy and paste.** `desktop` holds the clipboard: text first, then images and lists of files. Ctrl+C, Ctrl+X and Ctrl+V and the Edit menus in every `uikit` app and the terminal. An app reads the clipboard only when the person pastes into it, through keys `desktop` sees on the trusted path; a renderer never holds it, so a page reads it only through the browser on a paste. Kai's `clipboard_read` is a `Read` row returning `Data` labelled with the app it came from; `clipboard_write` runs, with `Compensate`. No clipboard history before v1.0 | G-core, A-core's turn for the rows, C for WIT | – | M | `desk-clipboard`: text copied in Editor pastes in Terminal, Files and the browser; an app that did not get a paste reads nothing |
| D-2 | **Notifications.** A `kai:desk/notify` import; a notification centre opened from the taskbar's clock, as in Windows 11; a per-app switch and Do not disturb in Settings; Kai's own notices there too. A notification only opens its app: it carries no button that acts | G-core, C | – | M | `desk-notify` |
| D-3 | **Screenshots.** Win+Shift+S and Print Screen: a region, a window or the screen, drawn by `desktop` on the trusted path, saved to `/home/user/Pictures/Screenshots` and put on the clipboard | G-core | D-1 | S | `desk-screenshot` |
| D-4 | **Open with.** Packages declare the file types they open (ADR 0011's manifest); Files gains "Open with"; Settings sets the default app per type | G-core, AP-2 | M-2, M-3 | M | `desk-open-with` |
| D-5 | **Drag and drop** of files and text between windows, through `desktop`; a drop is the person's act on the trusted path | G-core | D-1 | M | `desk-dnd` |
| D-6 | **Snapping** windows to halves and quarters, and Windows 11's snap layouts | G-core | – | S | Tour shots |

### 6.3 Track M, media and documents (pictures, PDF and sound in v0.2)

| # | Task | Lane | Needs | Size | Exit check |
|---|---|---|---|---|---|
| M-1 | **`kaimedia`**: PNG, JPEG, GIF and WebP decoding behind one API with size ceilings (at most 64 megapixels, est.), shared by the browser and the apps; `uipaint` draws images as themselves | MD, G-core's turn for `uipaint` | – | M | Host goldens; fuzz targets per format |
| M-2 | **Photos**: open, browse a folder, zoom, rotate, set as wallpaper | MD | M-1 | M | `app-photos` |
| M-3 | **PDF viewer** on hayro 0.9.0 (Apache-2.0 OR MIT), the document parsed in its own sealed process like a renderer, with a 384 MiB ceiling (est.; G13 peaked at 272 MiB) or hayro's caches bounded, and the OS's fonts given for the standard 14; a password-protected file asks for its password; find, zoom, print to PDF later | MD | M-1 | M | `app-pdf`; a fuzz target |
| M-4 | **Sound**: `audiod` (ADR 0007) on QEMU's virtio-sound first, through virtio-drivers 0.13 (MIT, already in the kernel) as P6-S1 did, then Intel HDA in P7; mixing, a volume per app, the taskbar's volume control | K's turn, S-io | – (P6-S1 passed) | L | `audio-out` |
| M-5 | **Music**: Ogg Vorbis, Opus and FLAC, a library view and playback controls; decoders picked in the PR from ADR 0009's free class (symphonia is MPL-2.0, so it would be an ask) | MD | M-4 | M | `app-music` |
| M-6 | **Video**: WebM with VP9 or AV1 and Opus, the same decoders as B-17 | MD | M-4, B-17 | L | `app-video`: S06-2's bars |
| M-7 | **Zip files** in Files: open, extract, make, through zip 9.8.2 (MIT), with ceilings against zip bombs | AP-2 | – | S | `files-zip`; a fuzz target |

### 6.4 Track H, hardware and install (P7 and on)

P7's work, designed in ADR 0007. Listed here so the gaps have owners; P7's own plan sizes and orders it.

| # | Task | Where | Gate |
|---|---|---|---|
| H-1 | Installer to the PC's disk, with partitioning and a recovery slot (row 7) | P7, P8 for polish | Installs and boots on the founder's PC |
| H-2 | Power off through ACPI's \_S5, sleep (S3), battery and lid on laptops (row 11) | P7 | Power off and resume cases on real hardware |
| H-3 | USB sticks and drives over xHCI mass storage, FAT32 and exFAT read and write (row 10) | P7 | `usb-storage` case |
| H-4 | Wi-Fi, one chip family first (row 8). Firmware blobs are an ask (ADR 0009) | P7 | Joins a WPA2 and a WPA3 network |
| H-5 | Bluetooth: keyboards, mice, headphones (row 9) | After P7 | Pairs and plays |
| H-6 | Several displays (row 12), with `deskgeom` laying out more than one screen | P7 | Two-monitor tour shot |
| H-7 | GPU acceleration (row 13) | Later | Its own ADR |
| H-8 | Camera and microphone, with an always-visible indicator and an ask per app (row 33) | After P7 | Its own case |

### 6.5 Track W, work apps (late P6 and P7)

| # | Task | Lane | Needs | Size | Exit check |
|---|---|---|---|---|---|
| W-1 | **Calculator, clock and alarms** (row 20); alarms through D-2 | AP-2 | D-2 | S | `app-calc`, `app-clock` |
| W-2 | **Mail** (row 17): IMAP and SMTP over `kainet`; HTML mail drawn by `webcore` in a sealed renderer with remote images off until asked. Mail content is `Data`; every send asks (ADR 0002) | W-a, N-t | B-6, F-4 | L, several PRs | `app-mail` against a mock IMAP server |
| W-3 | **Calendar and contacts** (row 18): `.ics` and `.vcf` files, then CalDAV and CardDAV | AP-2 | W-2's network part | L | `app-calendar` |
| W-4 | **Office documents** (row 19): `.docx`, `.xlsx` and `.odt` opened read-only first, then edited. Until then track V's LibreOffice covers it, if question 1 keeps the VM | AP-2 | – | L, several PRs | Golden documents |
| W-5 | **Printing** (row 22): print to PDF first, then IPP Everywhere printers on the network | AP-2, N-t | M-3 | M | A mock IPP printer |

### 6.6 Track L, languages and input

| # | Task | Lane | Needs | Size | Exit check |
|---|---|---|---|---|---|
| L-1 | **Time zones with daylight saving** (row 30) from the IANA tz database (public domain), shipped as data and updated with the OS | K, G-core | – | S | Host tests over known changes |
| L-2 | **Keyboard layouts** (row 28): UK, German, French, Spanish, Nordic and others in `deskcore`'s keymap, chosen in Settings and switched from the taskbar | G-core | – | M | Host tests per layout |
| L-3 | **Fonts for every major script** (row 29): Noto CJK, Arabic, Hebrew, Devanagari and others (OFL-1.1, approved for fonts), shared with B-8 | G-core | – | M | Goldens per script |
| L-4 | **Right-to-left text** and bidirectional layout in `uikit`, through Parley's bidi | G-core | L-3 | M | Goldens |
| L-5 | **Input methods** for Chinese, Japanese and Korean (row 28) | G-core | L-3 | L | By P8 |
| L-6 | **The OS in other languages**: strings moved to translation files; Kai answers in the person's language as far as the model can | G-core, model lane | – | M | One second language end to end |

### 6.7 Track U, people and safety

| # | Task | Lane | Needs | Size | Exit check |
|---|---|---|---|---|---|
| U-1 | **Lock screen and sign-in** (row 23) with a passphrase or PIN, drawn by `desktop` on the trusted path; locks on idle and on Win+L | G-core, K's turn | – | M | `desk-lock` |
| U-2 | **Backup to another disk** (row 26): kaifs snapshots sent to a USB disk and restored from it | S-io | H-3 | M | Restore case |
| U-3 | **A password store** for the browser's autofill, sealed under the lock passphrase. Kai never reads it (ADR 0014 item 10). Its own short ADR first | A-core, W-a | U-1 | M | Its ADR's cases |
| U-4 | **Several people and accounts** (row 24) | P8 | – | – | P8's plan |
| U-5 | **Disk encryption** (row 25) | P8 | – | – | P8's plan |

### 6.8 Track A, accessibility

| # | Task | Lane | Needs | Size | Exit check |
|---|---|---|---|---|---|
| A-1 | **Keyboard only**: every app and the desktop usable without a mouse, with visible focus | G-core | – | M | A keyboard-only tour |
| A-2 | **Text size and zoom** across all apps and the browser | G-core | – | S | Goldens at 100%, 150%, 200% |
| A-3 | **Magnifier** | G-core | – | M | Tour shot |
| A-4 | **Screen reader** (row 27), reading the accessibility tree that already exists. It needs a voice: espeak-ng is **GPL-3.0** (its repository), so a voice is a founder question when A-4 starts; a voice under a free-class licence is the default to look for | G-core, model lane | – | L | By v1.0 |

### 6.9 Tracks S and V, apps from others

- **V, the Linux VM** (row 32): ADR 0006's work order, items 1-13, unchanged, as one track. It starts when a lane is free after B1 if question 1 says yes. Its gates are ADR 0006's exit test and spikes S06-1 to S06-3. **Test plan (S06-1 and G16, section 13.2):** each backend is tested under KVM on its own runners, SVM with NPT and NRIPS on the AMD runners and VMX with EPT and unrestricted guest on the Intel ones, and SVM is tested under TCG (NPT, no NRIPS) on every runner. A case states the backend it needs, and a runner of the other vendor skips it under KVM with a named reason, never a pass. Firefox in the VM registers as Kai's second browser provider (ADR 0014, "Changes to recently merged and open work").
- **S, others' apps in the store** (row 31): after v1.0, as ADR 0011 question 5 says. Nothing in P6.

## 7. Sessions and hot-file turns

### 7.1 Running sessions in parallel

| Session | Lanes | Tasks | Starts |
|---|---|---|---|
| 1 | W-e | B-1, then B-2 on G10's harness (S14-1, S14-3 and S14-4 are done) | At approval; B-1 on main needs question 2 |
| 2 | C | The rest of S14-2, F-0, then F-1, F-3 and, with K, F-2 | At approval; F-1 and F-2 on main need question 5 |
| – | G-core | D-1 as its next item, then D-2 and D-3; F-5 | When its current item lands |
| 3 | W-n | F-4 with N-t, then B-3, F-6, B-4 | When S14-1 has passed |
| 4 | MD | M-1, then M-2 and M-3; M-4 with K | When a session is free |
| 5 | W-a | B-6, then B-9 with W-n | At B0 |
| – | A-core, model lane | B-10 and D-1's rows | At B2 |
| – | V | ADR 0006's work order | After B1, if question 1 keeps it |

Two sessions start at approval. Others join as lanes free up; you set the number.

### 7.2 Where P6 takes a hot-file turn

| File | P6's change | Task | Turn |
|---|---|---|---|
| `kabi/src/plan.rs` | Plan lines for `webnet` (`netdest=proto:tcp,ports:443+80,hosts:any`, question 3), `webview` and `audiod` | B-4, B-5, M-4 | K reviews |
| `kabi/src/handle.rs`, `kernel/src/handle/` | Child surfaces; signed code images | F-5, F-2 | Lane K |
| `kernel/src/process/` loader | Mapping a signed `.cwasm` read-execute | F-2 | Lane K |
| `surfabi/`, `deskcore/`, `user/src/bin/desktop` | Child surfaces, the clipboard, notifications, screenshots, snapping, the lock screen | F-5, D-1 to D-6, U-1 | G-core |
| `wit/` | `kai:desk` clipboard and notify imports | D-1, D-2 | Lane C |
| `agentabi/`, `shellcore/` | The `browser_*`, `media_*` and `clipboard_*` rows | B-10, D-1 | A-core |
| `llmproto/` | Those rows in the pack (ADR 0010's budget) | B-10, D-1 | The model lane, with the real-model gates |
| `uikit/`, `uipaint/` | Images drawn as themselves; right-to-left text | M-1, L-4 | G-core |
| `deny.toml`, `NOTICES.txt`, `xtask/notices.json` | Question 2's MPL-2.0 exceptions; the Noto fonts | B-1, B-8 | Lane L reviews |
| `Cargo.toml`, `Cargo.lock`, `docs/decisions.md` | New crates and decision rows | each | ADR 0000 §4.2: re-run cargo, append only |
| `.github/workflows/` | New `web-wpt.yml` and `web-sites.yml`; cases added to `ci.yml`'s shards | B-2, B-15, B-18 | The manager reviews |

## 8. Decided in this plan

No ADR settles these. Each is small and can change in review.

1. **P6 lands on main.** v0.1 fixes stay on `release/v0.1`, as in P5.
2. **One media crate.** `kaimedia` decodes images, and later sound and video, for the browser and the apps alike, so one fix covers both.
3. **The clipboard is the desktop's.** Only the person's paste reads it; renderers never hold it; Kai reads it only through a `Read` row whose answer is `Data` (D-1).
4. **Real sites only on GitHub runners, never committed.** The site list (section 4.4) is loaded live by `web-sites.yml`. The run keeps each page's text and one screenshot as artifacts for 30 days; no page is ever committed to the repository, for copyright and privacy. No CI boot case reaches the internet.
5. **Upstream test suites are fetched, not vendored.** WPT and test262 are fetched at a pinned commit in CI and never committed or shipped.
6. **Pass-rate bars move only with a row.** A gate in section 4.2 changes only by a decisions row with the manager's OK.
7. **Every parser of outside bytes is fuzzed** from its first PR (section 10).
8. **New apps follow Kai Desk** (ADR 0005), and their windows have a full accessibility tree from their first PR.
9. **No new engine JITs before v1.0** (ADR 0003 §1, ADR 0012). Code runs interpreted or compiled ahead of time in CI.

## 9. Spikes

None runs a model on the founder's PC, and none runs QEMU there.

| Spike | ADR | Where | Length (est.) | Gates | When |
|---|---|---|---|---|---|
| **S14-1** `engine-in-wasm` | 0014 | GitHub runner | About 1 week | Where renderers run (Wasmtime or native); B-1 | **Passed** (section 13.2) |
| **S14-2** `wasmtime-nostd` | 0014 | Host and a KVM boot case on GitHub | About 2 weeks | F-1, F-2 | **Partly done**: signed native code runs; the component model and the figures remain (section 13.2) |
| **S14-3** `js-engines` | 0014 | GitHub runner | About 1 week | Boa or QuickJS-NG; B-11 | **Done**: QuickJS-NG (section 13.2) |
| **S14-4** `web-compare` | 0014 | GitHub runner, read-only | Days | B-15 and the site-list gates | **Done**: 10 of 12 stable; gates compare in one job (section 13.2) |
| **P6-S1** `audio-virtio` | 0007 | A KVM boot case on GitHub with QEMU's virtio-sound to a null backend | Days | M-4's first driver | **Passed** (section 13.2) |
| **S06-1** to **S06-3** | 0006 | As ADR 0006 says | 2-5 weeks in all | Track V | S06-1 **done, its gate failed**: VMX joins P6; G16 found EPT and unrestricted guest on every Intel runner (section 13.2); S06-2 and S06-3 when V starts |

ADR 0014 gives S14-1 to S14-4's build, measure, pass and fail. **P6-S1** measures whether a `no_std` virtio-sound driver plays 60 s of 48 kHz stereo with no underrun under KVM; if it cannot, M-4 starts on emulated Intel HDA instead.

## 10. Tests

- **Host goldens** for `webcore` (pixels per fixture page), `kaimedia`, the PDF viewer and fonts per script.
- **WPT** reftests and testharness tests, and **test262**, at pinned commits, on GitHub runners (`web-wpt.yml`); each run records its pass rates in its PR.
- **The site list** against Firefox headless (`web-sites.yml`, section 8 item 4).
- **Fuzz targets** in `fuzz/`, run by `.github/workflows/fuzz.yml` on demand, their bodies also run in host tests as in P5: `html`, `css`, `font`, `woff2`, `png`, `jpeg`, `gif`, `webp`, `http_response` (grown), `cookie`, `url`, `psl`, `pdf`, `zip`.
- **Hostile tests** for `webnet`: a renderer asking for another site's cookies or a cross-site protected response is refused.
- **Boot cases** under KVM and TCG against xtask's HTTPS mock: section 1's list, plus `wasmtime-hello`, `code-image`, `desk-child-surface`, `desk-open-with`, `desk-dnd`, `desk-lock`, `files-zip`.
- **Real-model runs** only on GitHub, at B-10 and D-1: fluency at least 47/50, actbench with browser tasks, and S02-1's bait probes extended with page-injection probes (a page, a PDF and a download that each tell Kai to send something), at 0 false runs.
- Each run records its runner and run id in its PR.

## 11. Crates and licences

ADR 0014's "Crates and licences" has the full table for the browser, each licence read from crates.io's published metadata on 2026-11-23, and from the repositories' licence files for C libraries, fonts, data and test suites. In short:

- **No GPL or LGPL code is in anything this plan adopts.** `cargo deny` fails CI on any licence outside `deny.toml`'s allow list, so nothing deeper in the graph lands unseen.
- **Needs your OK (question 2):** 15 MPL-2.0 crates from Servo and Mozilla: stylo, stylo_dom, stylo_traits, stylo_derive, stylo_static_prefs, stylo_atoms, stylo_config, selectors, cssparser, cssparser-macros, dtoa-short, to_shmem, to_shmem_derive, app_units, uluru; and the Public Suffix List's data (MPL-2.0) inside `psl` (whose own code is MIT/Apache-2.0). P5's question 3 left the suffix list out by default; this asks again because the browser needs it.
- **Asked only if a spike needs it:** mozjs and StarlingMonkey (SpiderMonkey, MPL-2.0), if both QuickJS-NG and Boa later fall short on the site list (S14-3 did not need them).
- **Free, no approval:** Blitz's crates, html5ever, Taffy, Parley, vello_cpu, Boa (Unlicense OR MIT), rquickjs and QuickJS-NG (MIT, chosen by S14-3; wasi-libc, linked in, is MIT and Apache-2.0 by its repository's licence files), wasmtime (Apache-2.0 WITH LLVM-exception), brotli, h2, tungstenite, png, gif, image-webp, zune-jpeg, wuff, resvg, rav1d, libvpx and libopus (BSD-3-Clause), hayro (Apache-2.0 OR MIT), zip (MIT). The Noto fonts are OFL-1.1, already approved for font data.
- **Flagged, not adopted:** GStreamer (**LGPL-2.1**, Servo's media) and FFmpeg (**LGPL**, Ladybird's media and the guest's H.264, ADR 0006 question 7); EasyList (**GPL-3.0-or-later OR CC-BY-SA-3.0**, question 8); espeak-ng (**GPL-3.0**, a possible screen-reader voice, asked when A-4 starts); symphonia (MPL-2.0, an ask only if M-5 picks it). The Linux guest's GPL parts are ADR 0006 question 1's.

## 12. Estimate, risks and out of scope

### 12.1 Estimate

All person-months (PM) are estimates for one engineer.

| Part | PM (est.) | Source |
|---|---|---|
| F, foundations | 3-5 | ADR 0014 "Where the engine runs" (2-3) plus F-4 to F-6 |
| B to B4 (v0.2) | 13-22 | ADR 0014 option B: 6-10 to B3, the rest of 18-30 to B5 |
| B5 and B6 | 8-12 | The rest of option B's figure, and B6's media |
| D | 3-5 | Section 6.2's sizes |
| M | 6-10 | Section 6.3's sizes |
| W | 8-14 | Section 6.5's sizes |
| L | 4-8 | Section 6.6's sizes |
| U before P8 | 2-4 | U-1 to U-3 |
| A | 4-7 | Section 6.8's sizes |
| V | 8-17 | ADR 0006, option C, plus the VMX backend S06-1's fail rule moves into P6 (+1-2). G16 keeps the +1-2: every Intel runner offers EPT and unrestricted guest, so the VMX backend needs no shadow paging and no real-mode emulation |
| H | P7's plan | ADR 0007 |

**v0.2's part** (F, B to B4, D-1 to D-3, M-1 to M-4): about 25-40 PM (est.). With four to five sessions in parallel and the founder's "good and slow over fast", that is about 8-12 months from approval (est.). The browser sets that date. Everyday-OS parity as a whole runs past v1.0; this plan is revised at each milestone.

**Start with these two, in parallel, once the plan is approved:**
1. **B-1** (session 1), on question 2. Host and GitHub only.
2. **The rest of S14-2 and F-0, then F-1** (session 2), on question 5. One KVM boot case on GitHub.

G-core takes D-1, the clipboard, as its next item.

### 12.2 Risks

| Risk | What we do |
|---|---|
| Blitz slows down or changes direction; it has had no stable release since 2025-10 | We pin its beta and may carry a fork (ADR 0014 item 2). Stylo and html5ever are Servo's and Firefox's and move on regardless. Servo is re-measured at B5 |
| Renderers in Wasm are too slow | S14-1 decides before B-1; native renderers with a `std` port are the costed fallback |
| QuickJS-NG falls short on real sites, or Temporal is late | Boa is the measured fallback (S14-3); SpiderMonkey only on question 2's second part |
| Memory: 8 renderers at 256 MiB beside the model | The roadmap puts M10b (demand paging) before P6. Until it lands, the renderer limit is 4 (est.), and B1's gate measures memory |
| Prompt injection from pages, PDFs and downloads | All of it is `Data` (ADR 0002 D6); `browser-pf` and the page-injection bait probes gate B2; forms, sends and downloads ask |
| A renderer escape | Two walls (Wasm and the process); the renderer holds only a site-tagged route and a surface; fuzzing from the first PR; P8's security review |
| Cookies leak between sites | Per-site jars on the suffix list (F-6); `webnet`'s hostile tests |
| The long tail of web compatibility | WPT and the site list gate each milestone; track V's Firefox covers the heavy web meanwhile, if question 1 keeps it |
| Upstream security fixes arrive late | A monthly sync and a 14-day target for security fixes (ADR 0014 item 11) |
| GitHub Actions time | WPT and the site list run on changes to `webcore`, `webjs` and `webnet` and nightly, not on every PR |

### 12.3 Out of scope, and where it goes

- **Not before v1.0:** a JIT for JavaScript or Wasm; WebGL and WebGPU (they wait for H-7); browser extensions; sync of bookmarks and history across PCs; WebRTC; push notifications from sites.
- **Never in a browser of ours:** Widevine and other DRM, so Netflix, Disney+ and their like work only where a browser that licenses DRM runs. Track V's Firefox could carry Widevine only through Google's licence, which is not planned.
- **H.264 and AAC:** off until question 9 is asked again at B6.
- **P7:** track H (ADR 0007). **P8:** accounts, disk encryption, the security review of `webview`, `webnet` and the code loader, fuzzing at scale.
- **After v1.0:** others' apps in the store (track S).

## 13. Groundwork results (2026-11-26)

While this plan waited for approval, the manager handed out the work that needs none (ADR 0000:43 and :298): spikes on `spike/*` branches, which are never merged, and host-only crates. The plan's sections above already use what follows.

### 13.1 What landed or is queued

| Task | What | State | What it does for this plan |
|---|---|---|---|
| G7 | `svm`: the VMCB, exits, intercepts, EVENTINJ and NPT entries, host-tested (ADR 0006 item 2) | Merged, #1449 | Track V's first crate is on main |
| G9 | `kainet`'s `Fetch::exchange`: methods, headers, streamed bodies, gzip and brotli, keep-alive | Merged, #1439 | F-4's host half. TLS 1.2 (question 4) is what remains |
| G11 | `tzcore`: time zones with daylight saving from TZif and TZ strings | Merged, #1443 | L-1's host half. Settings and the clock remain |
| G12 | UK, German, French, Spanish and Nordic keyboard layouts as data | Merged, #1437, #1456 | L-2's host half. Choosing one in Settings remains |
| G8 | `kaimedia`: one decode API over PNG, JPEG, GIF and WebP, with ceilings | Queued, #1448 | M-1's host half. `uipaint` drawing images remains |
| G10 | `cargo xtask wpt`: reftests through a pluggable renderer at a pinned commit | Queued, #1450 | B-2's harness. It needs B-1's renderer to report a baseline |
| G14 | `kaizip`: list, extract and make, with bomb, escape, duplicate and encryption ceilings | Queued, #1447 and #1452 | M-7's host half. Files' menu remains |
| G15 | S06-1's census on main: `cpufeat::Nesting`, the `nesting:` boot line, case `nesting-census`, and `nesting-census.yml` by dispatch only (P6 row 1) | In review, #1489 | Track V's runner choice, re-run when the pool may have changed |

### 13.2 What the spikes found

| Spike | Result | What it changes |
|---|---|---|
| **S14-1** `engine-in-wasm` (#1441, run 45720633850) | **Passed.** Blitz 0.3.0-beta.4 with Stylo 0.22 built for `wasm32-wasip2` with no patches. Ahead-of-time code is 1.62x native (bar 2x). The article fixture reached first paint in 107 ms (bar 400 ms). Peak guest memory was 30 MiB (bar 256 MiB). Without `simd128` it is 2.50x. wasmi is 36.7x and Pulley 57x. Layout is recursive: 200 nested blocks trap with 1 MiB of stack | Renderers run on Wasmtime ahead of time, built with `simd128` (ADR 0014, "Where the engine runs", option ii). wasmi is ruled out for pages. F-1 carries the stack settings and a nesting cap |
| **S14-2** `wasmtime-nostd` (#1438) | **Partly done, working so far.** Wasmtime 51's runtime alone builds for our ring-3 target, with about 60 lines of platform glue. A `.cwasm` signed with ed25519 is checked, then run natively from pages a throwaway MEM_EXEC call made read-execute, and on Pulley. A flipped bit is refused. Native code is 7x faster than Pulley. target-lexicon cannot parse `x86_64-kai-user`. Still to do: the component model, the pass figures against the bars, and traps | F-0 is added (the user target's name). F-2's loader is the spike's MEM_EXEC done properly. Question 5 is still needed to put it on main |
| **S14-3** `js-engines` (`spike/js-engines`, run 45720308618) | Without `built-ins/Temporal`, test262 is QuickJS-NG 95.96% and Boa 94.88%. With it, Boa 95.35% and QuickJS-NG 86.88% (QuickJS-NG has no Temporal). On Wasmtime ahead of time, QuickJS-NG's Octane score is 5.2-7.5x Boa's. The `.wasm` is 1.26 MB against 7.37 MB. Start-up is the same: about 6 ms a run. Neither needs a JIT | By ADR 0014's rule (the fastest of the two at 90% or more) **QuickJS-NG is taken**, reading the bar without Temporal. Its C is built by wasi-sdk in CI, which ADR 0006 question 2 allows. Boa stays the fallback, and S14-3 does not reach SpiderMonkey's trigger. Temporal is added to QuickJS-NG, or in our own code, by B4 |
| **S14-4** `web-compare` (`spike/web-compare`, run 45732633843) | 10 of 12 tier-1 sites gave the same text on two runs. Hacker News (92.6% of words shared) and DuckDuckGo's results (66.9%) change live. With every cookie refused, Firefox read 0 words from Stack Overflow and 17 from BBC News | Each site gate compares our browser with Firefox **in the same job**, not across runs (section 4.2, B1). Stack Overflow and BBC News move to "tracked, not gated" (section 4.4) |
| **S06-1** nesting census (`spike/nesting-census`, run 45734622772) | **The gate failed:** only 8 of 20 KVM runs had nested SVM with NPT, all of them on AMD hosts. The other 12 were Intel hosts with nested VMX. Under TCG, SVM with NPT was offered in 20 of 20 runs, without NRIPS | ADR 0006's fail column applies: the VMX backend moves into P6 (+1-2 PM, section 12.1). Track V tests SVM under KVM on AMD runners and under TCG on all of them |
| **G16** VMX capability MSRs (`spike/nesting-census`, run 45824528913) | **All 12 Intel runs** (11 Xeon 6975P-C, 1 Xeon Platinum 8370C) offer nested VMX with EPT and unrestricted guest. VMXON is allowed (FEATURE_CONTROL `0x100005`, locked). VPID, true controls, 4-level EPT walks, write-back EPT, 2 MiB and 1 GiB EPT pages and EPT accessed and dirty flags are all offered. The 8 AMD runs had SVM with NPT and NRIPS again; TCG had SVM with NPT, no NRIPS, in 20 of 20. The 8370C allows fewer controls than the 6975P-C | Each backend can be tested under KVM on its own runners, SVM on AMD and VMX on Intel, and SVM under TCG on all (section 6.9's test plan). The VMX backend needs neither shadow paging nor real-mode emulation, so its +1-2 PM holds (section 12.1). `vmx` requires only the controls both Intel models allow |
| **P6-S1** `audio-virtio` (#1454) | **Passed.** 60 s of 48 kHz stereo played over virtio-drivers' sound device with 0 underruns and about 1.3% CPU (debug build) | M-4 starts on virtio-sound. The emulated-HDA fallback is not needed |
| **G13** PDF on hayro (`spike/hayro-pdf`, run 45728927723) | 19 of 20 PDFs rendered; the one with a user password was refused, as poppler refused it. About 15x faster than pdftoppm. Vector, transparency, CJK and forms differ from poppler on 0.2-2.6% of pixels. Standard-14 text differs on about 8%, because of hayro's own substitute fonts. Peak memory was 272 MiB, against 27-48 MiB for ordinary files | M-3 keeps hayro, with a memory ceiling, the OS's fonts for the standard 14, and a password ask |

### 13.3 Rechecked against main at 42869af8

- **Power off works.** POWER_OFF (call 134) turns the machine off through the firmware's ResetSystem (since 2026-11-18). ACPI's \_S5 through uACPI comes later (kernel/src/process/syscall/power.rs:17). Gap row 11 is corrected.
- **The KRNG v1 ring is on main** (`kabi/src/ring.rs`): one ring format for bulk data over an `Shm`. Child surfaces (F-5), renderers' bodies through `webnet` (ADR 0014 item 5) and ADR 0006's `VMEXIT` ring use it.
- **`Hold`.** The roadmap still says "P5 closed, but for `Hold`". `net_send` and the hold case (#990, landed 2026-11-25) are on main. The roadmap's P5 line is rechecked when P5 is closed. Nothing in this plan waits on it.
- **The shell's destination** is named in `kabi/src/plan.rs:96` (`SHELL_NET`, `proto:tcp,ports:443,hosts:any`). `webnet`'s line will sit beside it.
