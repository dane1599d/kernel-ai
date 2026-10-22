# ADR 0005: Graphics: compositor, surface protocol and visual direction

Status: Proposed, 2026-10-21. The founder settled the font and Unicode-3.0 licences on 2026-10-22 (question 2). The design and the look wait for approval.

Repo citations are `path:line` on main at `ec5ecff`. Open work is cited at its branch tip: #18 virtio-gpu (`origin/dane/virtio-gpu-a5daaa`, `7603e54` plus merge `af7635d`), #19 USB input (`origin/dane/usb-input-83eb7c`, `0717caa`) and #16 networking (`origin/dane/net-dhcp-dns-b5bdb7`). Nothing was built or run for this ADR, and no model ran anywhere. Crate versions, licences, features and dependencies were re-read on crates.io on 2026-10-21 from the founder's PC (web reads only), as were the repo citations against `ec5ecff`; contrast ratios are WCAG 2.2 arithmetic.

## Context

What draws the screen today:
- Only the kernel draws. The console paints the `termcore` grid in #17's "Ember" theme (kernel/src/console/mod.rs:1-12; theme.rs:1), straight into Limine's framebuffer (kernel/src/console/fb.rs:1-7). It checks only that pixels are 32-bit and ignores the colour masks (kernel/src/console/mod.rs:117-119).
- Ring 3's only output is WRITE, which prints text (kabi/src/call.rs:6). There is no shared memory, and a message is at most 8,192 bytes (kabi/src/lib.rs:160). None of the nine capability bits is for display or input (kabi/src/lib.rs:82-108).
- User pages are never unmapped, and the kernel sends no TLB shootdown (kernel/src/memory/user.rs:5-12). The one mapping shared with ring 3 is the model window: 2 MiB pages linked into a whole PML4 slot, counted nowhere and unlinked only when the space dies (user.rs:16-20, :183). A futex wakes only the caller's own threads (kabi/src/call.rs:108-110). There are at most 16 processes (kernel/src/process/mod.rs:54).
- The kernel may not use SIMD (docs/decisions.md:138), and most of our own crates build unoptimised in dev (Cargo.toml:14-18). A ring-3 program on the hard-float target may use SSE2, and AVX2 per function after a CPUID check (docs/decisions.md:138, :140); today that is `llm`, `probe`, `vecstress` and `workers` (xtask/src/userbuild.rs:24).

What the rest of the plan sets:
- Apps are WebAssembly on wasmi, about 10x slower than native (docs/research/live-editable-system.md:54), so rasterising and blitting stay native (:102). [ADR 0003](0003-app-model.md) runs one hard-float `comphost` per app and leaves the body of `kai:ui@0.1.0` to this ADR.
- The model is text-only. Its context is 2,048 positions today (kabi/src/model.rs:65) and 3,072 from P1 ([ADR 0010](0010-skill-pack.md)). In OSWorld, filtered accessibility trees needed a context of about 6,000 tokens to fit 90% of single observations, so the agent's view must be far smaller.
- P0 asks for "the compositor and surface protocol, weighed against Wayland so P6's Linux apps can display without a rewrite" (docs/roadmap.md:139). P3's exit test includes "The agent opens apps and acts inside them" (roadmap.md:142). The app ABI is versioned from its first release (roadmap.md:131).
- Plain requests should just happen (docs/decisions.md:203, :205). `:apply` is confirmed at "a prompt the kernel draws" (decisions.md:94).
- [ADR 0006](0006-existing-software.md) runs Linux apps in a micro-VM whose own compositor, `cage`, draws the guest screen; in v0.2 that screen is one foreign surface here.

Work this ADR builds on:
- #18 adds `kernel/src/display`: a back buffer of `0x00RRGGBB` pixels with `flush(rect)`, our own polled virtio-gpu 2D driver, a GOP fallback and a 64x64 hardware cursor (branch kernel/src/display/mod.rs:1-12, :141-179). Its decision row holds the only guest timing: under local TCG a full 1920x1200 flush took 0.5-1.9 ms and one text row 0.1-0.5 ms (branch docs/decisions.md:197). It also says VirtualBox has no virtio-gpu; that was not checked against VirtualBox's manual.
- #19 adds USB keyboards, mice and tablets. Keys become PS/2 set-1 bytes, and pointers feed a new event queue (branch kernel/src/usb.rs:1-5, kernel/src/pointer.rs:134-197).
- The founder's VirtualBox VM has VMSVGA with 16 MiB of VRAM and PS/2 input (xtask/src/vbox/mod.rs:330-331). That holds a 1440p frame (14.7 MB) but not a 4K one (33.2 MB).

This ADR decides the compositor, the surface protocol, how apps draw, how the agent sees and acts on windows, the trusted path on screen, and the look.

## Options considered

Four designs were judged; scores are the judge's, out of 10.

| Design | What it is | For | Against | Result |
|---|---|---|---|---|
| A. Own surface protocol, Wayland bridge later (8) | Wayland's objects, semantics and 8-byte header over our shared-memory rings and handles; a ring-3 compositor drawing with tiny-skia, skrifa and HarfRust; every window carries an AccessKit tree | P6 becomes a 1:1 translation. The agent view and the trusted panel are in the protocol from the first app. The renderer sits behind one crate. | Kernel work comes first, and A sized it too low: it planned to map buffers "as MODEL_MAP does" and had no user unmap. Two protocols for good. Bridged apps are opaque to the agent. We own a toolkit. Its `apphost` duplicated 0003's `comphost`. | **Base** |
| B. Wayland native from day one (4) | A Smithay compositor built as a static musl binary, run on a slice of about 45 Linux calls added to the kernel in P3; apps as Wayland clients | Best Linux-app fidelity; no bridge; Wayland's per-interface versions | Moves about 5,600 kernel lines of P6 into P3 before the first window. Decides 0006's question before its spike. Puts C (libxkbcommon, pixman) in the trusted display path. Smithay's latest release is 0.7.0 of 2025-06-24. Redox: "Smallvil performance on Redox is not adequate" (2025-11-30). Apps as `abi=linux` conflicts with 0003. | Lost. Kept: a display connection given at spawn |
| C. Orbital/rio-style compositor, proxy later (6) | `win` composites client buffers; text control requests (`/win/list`, outlines, `act`); a 32 px strip drawn by the kernel as the only place for asks | Smallest (about 10,500 lines); a text control plane suits Rhai and a 4B model; a per-window header page, so a frame needs no IPC | A 32 px strip cannot show a 40-line script (decisions.md:74). It contradicts [ADR 0001](0001-capabilities.md), which puts the trusted path in the compositor. It also planned window-linked buffers and a 1.21:1 window edge. | Lost. Kept: the text view of windows, the takeover rule, the 4 KiB header page, the latency scene |
| D. Visual direction (8) | "Ember Workbench" (dark-first, Ember tokens, Inter and JetBrains Mono, a bottom `kai>` line as the AI panel) against "Daylight Document" (light-first, Atkinson type, an AI column on the right) | Continues the Ember look the founder has seen (docs/images/terminal-after.png). Every contrast figure checks. Contrast becomes a CI test. | Text quality is a guess until screendumps exist. It named swash on skrifa 0.48, which swash 0.2.10 does not allow. | **The look**: Ember Workbench, with Daylight's Atkinson pair as the legible setting |

Open-source reuse weighed. Licences come from crates.io metadata or the LICENSE file, read 2026-10-21. "Free" is the founder's 2026-10-21 class (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib): no question, and each PR lists the crates.

| Part | Chosen | Rejected or deferred, and why |
|---|---|---|
| 2D raster | tiny-skia 0.12.0, BSD-3-Clause, free. Its dependencies are free too: arrayref (BSD-2-Clause), bytemuck (Zlib OR Apache-2.0 OR MIT), arrayvec, cfg-if, log, tiny-skia-path. | vello_cpu 0.3.0 (Apache-2.0 OR MIT): measured in S05-1 as the fallback; no x86 figures are published. pixman (MIT): C. |
| Fonts: parsing, outlines, hinting | skrifa 0.48.0 and read-fonts 0.45.1, MIT OR Apache-2.0. read-fonts 0.45 requires `yoke` 0.8 with its `derive` feature, which brings `zerofrom` and the build-time `yoke-derive` and `zerofrom-derive`, all Unicode-3.0: ask until the founder approves 0009's proposed free list. skrifa 0.49.0 and read-fonts 0.46.0 came out on 2026-10-21; harfrust 0.14.0 still takes read-fonts ^0.45, so the pin stays until harfrust moves. | FreeType: C. Chrome removed FreeType from Blink in Chrome 145. |
| Shaping | harfrust 0.14.0, MIT, with its `icu` feature off (on, it adds Unicode-3.0 crates) | rustybuzz 0.20.1: superseded by harfrust |
| Glyph raster | tiny-skia fills skrifa's outlines | swash 0.2.10 requires skrifa `>=0.31.1, <=0.44`: a second font parser beside harfrust's |
| Text layout | our own in v0.1: breaks at spaces, no bidi | parley 0.12.0: its engine requires skrifa ^0.44, harfrust ^0.12 and ICU4X (Unicode-3.0) as non-optional dependencies |
| Accessibility schema | accesskit 0.25.1 and accesskit_consumer 0.39.1, MIT OR Apache-2.0, no_std | AT-SPI: D-Bus and a round trip per query |
| Toolkit | our own `uikit` | Slint 1.18.1: GPL-3.0-only, a royalty-free licence written for applications, or commercial. iced 0.14.0 needs std. egui 0.37.0 is immediate mode. Xilem/Masonry 0.4.0 (Apache-2.0, pre-1.0): ideas only. |
| Compositor | our own | Orbital (MIT, 4,506 lines, bound to std, DRM and Redox schemes), Weston 16.0.1 (MIT), HaiWay (MIT per its README, 6,863 lines of C++; both line counts are the research's, not re-checked), Fuchsia Flatland (BSD-style): design references. niri (GPL-3.0-or-later), cosmic-comp (GPL-3.0-only), Nitpicker (AGPLv3 with a linking exception), Arcan (mixed): ideas only. |
| Linux display (P6) | 0006's guest runs `cage`; later a guest-side bridge | wayland-server 0.31.14, wayland-backend 0.3.17 and Smithay 0.7.0 (all MIT): only on 0006's `lxabi` fallback. Smithay 0.7.0 always depends on `xkbcommon ^0.8`, bindings to C libxkbcommon, whose files carry MIT, X11, ISC, HPND, HPND-sell-variant, MIT-open-group and one custom notice (`LicenseRef-anton-zinoviev`). |
| Fonts | Inter 4.1, JetBrains Mono 2.304, Atkinson Hyperlegible Next: SIL OFL-1.1, ask class | Geist (OFL) and IBM Plex (OFL with the Reserved Font Name "Plex"): not needed |
| Box drawing | a port of Alacritty's `builtin_font.rs`, Apache-2.0, free (keep the notice and state the changes) | the fonts' own glyphs: joins vary by font |
| Icons | Lucide 1.61.0 (ISC, free; icons from Feather, `minus`, `square` and `x` among them, are MIT), rasterised at build time by resvg 0.48.1 (Apache-2.0 OR MIT) in xtask | no other icon set was weighed |

## Decision

Kernel AI gets its own small window system. One program, `desktop`, draws every window, the AI's command line at the bottom of the screen and every question the AI asks, so no app can fake a question or a "yes". Apps describe their buttons and fields as well as drawing them, so the agent can read and press them without screenshots. The look continues the dark "Ember" terminal you have seen, and Linux apps arrive in v0.2 as one window drawn by a small Linux VM.

In the options' terms: base design A; the look is design D's "Ember Workbench"; grafts are C's text view, takeover rule and per-surface header page, and B's connection given at spawn. Names follow the decision sheet: the judge's `surface@1` is `kai-surface 1`, and the judge's `ui.*` rows are snake_case.

### 1. Shape

- A ring-3 compositor, the program and service `desktop`, owns the scene, damage, focus, window decorations, the cursor, the AI panel and the agent's view. Its logic is in a host-tested crate, `deskcore`. It builds for the hard-float target and runs under the supervisor. Under the `classes` policy it is never labelled `background`, so background threads never preempt it on wake, though one already running keeps a 2 ms shield (docs/decisions.md:39, :177). `classes` has no frame deadline; S05-1's scene (f) shows whether one is needed.
- The kernel keeps the display and input drivers (virtio-gpu, GOP, PS/2; [ADR 0007](0007-drivers.md)) and its console. The console stays the boot, panic and recovery screen ([ADR 0008](0008-updates.md)). The kernel takes Ctrl+Alt+F12 before `desktop` sees it and switches between the console and the desktop.
- If `desktop` dies, the console shows and the supervisor restarts it. `uikit` apps reconnect and rebuild their windows from their retained trees, without restarting.

### 2. Kernel interface

New calls are named here, never numbered: they take the next numbers from 38 (RESOLVE is 37) in claim order when their PR lands. `Display` and `Input` are new kinds for 0001's table; each gets the next free ceiling bit in claim order.

| Object or call | What it does |
|---|---|
| Kind `Display`, held only by the service `desktop` running the program `desktop` | The screen |
| `DISPLAY_CLAIM(&mut DisplayInfo) -> Shm` | Stops the console painting. Returns width, height, stride, format (XRGB8888) and the EDID size in mm, and hands over the back buffer as a kernel-owned `Shm` (MAP, READ, WRITE), mapped in 4 KiB pages. On GOP the `Shm` covers the Limine framebuffer (write-combining unverified); `desktop` composes in its own RAM frame and only writes damaged rows to it. |
| `DISPLAY_FLUSH(rects, n <= 32)` | virtio-gpu transfer and flush per rectangle; nothing on GOP |
| `DISPLAY_CURSOR(image, hot)`, `DISPLAY_CURSOR_MOVE(x, y)` | The 64x64 hardware cursor on virtio-gpu. GOP has none, so `desktop` draws one. |
| Kind `Input`, `desktop` only; `INPUT_READ(buf, cap, timeout_ms)` | Drains the kernel's one input queue of 0007's 16-byte `InputEvent` (`time_ns`, kind Key/Rel/Abs/Button/Wheel, HID usage code, value), fed by the i8042 and xHCI now and by `usbd` through `INPUT_PUSH` later |
| `Shm` ([ADR 0001](0001-capabilities.md)) | Surfaces and connections. 4 KiB pages, charged to the creator, mapped with rights, revocable. `SHM_UNMAP` and revocation need user unmap with a TLB shootdown, which the kernel lacks today (user.rs:5-12). `link_window` is not used. |
| Doorbells | 0007's `RING_ATTACH`, `RING_NOTIFY` and `NOTIFY_WAIT` |

- When `desktop` exits, when Ctrl+Alt+F12 is pressed and while `:apply`'s card shows, the kernel revokes the display `Shm` and repaints its console. Revocation leaves a zero page behind (0001), so `desktop` keeps running.
- The judge proposed evdev key numbers. 0007 already defines `InputEvent` with HID usage codes, so there is one numbering, written from the USB HID Usage Tables. Turning it into evdev codes for a Linux guest is 0006's `vmm`; that table is written from published specifications, never copied from Linux's `input-event-codes.h` (GPL-2.0-only WITH Linux-syscall-note).

### 3. The protocol `kai-surface 1`

Crate `surfabi`: no_std, host-tested, fuzzed. It keeps Wayland's object model, so a Wayland client, a guest bridge or 0006's foreign surface translates 1:1.

- **Connection.** An app whose `kai-app 1` has a `use` line for `kai:ui` gets a `Service` route to `desktop` at spawn; native services such as `vmm` get it through the plan's `uses=` word (0001). That is B's spawn-time connection: no socket path and no lookup. Without the route a process cannot open a window. The client creates two 64 KiB `Shm` rings, one per direction, so a hostile app spends its own quota, and passes them to `desktop` with 0001's `CALL_H`. Each is a KRNG v1 ring ([ADR 0007](0007-drivers.md) owns the format) of kind MSG with 64 entries; each request names one message in the ring's data area.
- **Wire.** Wayland's 8-byte header (object id; opcode and size), then 32-bit arguments: int, uint, fixed 24.8, string, array, new_id, object. A message is at most 4,096 bytes (Wayland's `WL_MAX_MESSAGE_SIZE`). An unknown opcode or a bad size closes the connection. The one message that carries a handle, `pool.create`, goes by `CALL_H`, so the kernel checks the `Shm`'s kind and rights.
- **Formats.** XRGB8888 and premultiplied ARGB8888, the two that "all renderers should support" in Wayland (wayland.xml:321). Wayland's ARGB8888 is premultiplied (wayland.xml:532-536), so compositing blends premultiplied sRGB values.

| Object | Requests | Events | From |
|---|---|---|---|
| registry | bind | global | `wl_registry` |
| pool | create(Shm, size), buffer(offset, w, h, stride, format) | | `wl_shm_pool` |
| buffer | destroy | release | `wl_buffer` |
| surface | attach, damage_buffer, frame, set_opaque, commit | preferred_scale (n/120), enter, leave | `wl_surface`, fractional-scale-v1 |
| toplevel | set_title, set_app_id, set_min_size, set_max_size, ack_configure | configure(serial, w, h, states), close | `xdg_toplevel` |
| popup | position(anchor, gravity, offset), grab | configure, done | `xdg_popup` |
| seat | set_cursor(shape name) | focus_in, focus_out, modifiers | `wl_seat`, cursor-shape-v1 |
| a11y | tree_update(bytes), commit, bind_action(node, action) | action_request(node, action, value) | ours |
| panel | layer, exclusive_zone | configure | ours, like wlr-layer-shell; system programs only |

- **`Surface`.** A pool's `Shm` begins with one 4 KiB header page (C's): magic, version 1, size, stride, format, scale, the front buffer index, `commit_seq`, up to 32 damage rectangles (u16 x, y, w, h) and a ring of 64 events of 32 bytes. Events are frame done, buffer release, and this surface's input: an `InputEvent`, the position in surface-local logical units and, for a key, its text, so native apps need no keymap. A client draws into its back buffer, writes its damage, bumps `commit_seq` and rings the doorbell. `desktop` latches, composites, flushes and posts the events. A frame costs no message and no IPC call, only a doorbell. A translator maps `wl_surface.commit` to a header write.
- **Trees.** `a11y.tree_update` carries fixed little-endian records that mirror a pinned subset of AccessKit 0.25.1's `TreeUpdate`: role, name, value, bounds, actions, children and focus. An update may span several messages and applies at `a11y.commit`. `desktop` caps a window at 4,096 nodes and 256 KiB. An AccessKit upgrade changes a converter, not the ABI.
- **Focus.** Only `desktop` moves keyboard focus: by click, by Alt+Tab, or by a row the agent runs. A new window takes focus only with an activation token from a user action (the xdg-activation-v1 rule).
- **Versions.** Each object has a version, as in Wayland. A release only adds requests and events. `kai-surface 2` is kept for GPU buffers.

### 4. Apps and `uikit`

- An app builds a retained view tree through `kai:ui@0.1.0` (0003 owns the package and its freeze rule; this ADR owns the body). `uikit` runs as native host functions inside `comphost`. It lays out the tree, draws it through `gfx2d` into the window's `Surface` and emits the AccessKit tree. The widget tree is the accessibility tree, so an app cannot show a control the agent cannot see. No pixels enter the app's 2,048 KiB linear memory (livecore/src/settings.rs:38).
- v0.1 widgets: label, button, text field, multi-line text, list, table, tabs, toggle, slider, menu, scroll view, split view, image and progress.
- Widgets take only tokens and logical px; apps get no AI colour. A control bound to a declared action shows that action's `doc` text from its `agentabi` row ([ADR 0002](0002-agent-actions.md)). What the user reads, what the agent's view holds and what the log says are the same words.

### 5. Rendering: `gfx2d`

- A no_std crate on tiny-skia 0.12.0 (`default-features = false`; `no-std-float`, `simd`), skrifa 0.48.0 (`libm`) and harfrust 0.14.0 (`default-features = false`; `libm`), all on one read-fonts 0.45.
- Glyphs: skrifa outlines filled by tiny-skia to 8-bit coverage, in an atlas keyed by (font, glyph, size in 1/64 px, x phase in quarters). Light vertical hinting at scale 1, none at 2 and above. Greyscale anti-aliasing only: subpixel breaks under scaling, compositing and screendumps. Each theme has a 256-entry coverage table; dark starts at c' = 255(c/255)^0.8 and light at identity, both set by eye from S05-1's screendumps.
- Box drawing and block elements (U+2500-259F) are drawn by code per cell, from the Alacritty port. Shadows are pre-rendered nine-slice images, so a moving shadow costs a blit.
- Compositing is damage-tracked: at most 16 rectangles a frame, their bounding box past that. Opaque regions are copied, the rest blended source-over. A 60 Hz timer drives frames only while something is damaged, since virtio-gpu has no vblank.
- No swash and no parley in v0.1. No code is generated at run time: SIMD paths are fixed at compile time or picked by CPUID, as the engine's are (docs/decisions.md:140).
- The judgement said Unicode-3.0 would arrive only with parley. crates.io shows read-fonts 0.45 already requires `yoke` (Unicode-3.0). If the founder does not approve Unicode-3.0, `gfx2d` pins skrifa 0.46.0 and harfrust 0.13.0 on read-fonts 0.43.3, which need none, and gives up later fixes.

### 6. How the agent sees and acts

`desktop` holds every window's tree and walks it with accesskit_consumer 0.39.1. It owns these rows in 0002's `agentabi` registry; scripts call them by the same names. 0002 derives each tier from the row's facts and inverse, which gives:

| Row | Does | Tier |
|---|---|---|
| `ui_windows()` | open windows: id, app, title, focused | run, read-only |
| `ui_view(window)` | the window's interactive and labelled nodes | run, read-only; returns `Data` |
| `ui_open(app, arg)` | opens a native app with an argument such as a path | run; undo closes it |
| `ui_focus(window)` | moves keyboard focus | run; undo restores it |
| `window_close(window)` | closes any window | run if the app reports nothing unsaved (undo reopens it), else ask |
| `ui_act(window, node, action, value)` | sends an AccessKit `ActionRequest` to the app, never synthetic input | the tier of the action the node is bound to; an unbound node asks |

- `window_close` is one row for every window, shared with 0006: for a Linux window `desktop` passes it to `vmm`, and the inverse is 0006's. It replaces the `ui_close` that 0002's phase table lists. 0006's `app_open` opens Linux apps; `ui_open` opens native ones.
- **The view** is the window list plus the focused window's nodes: short ids, role, label and value, at most 300 tokens counted with `bpe`, ending `+N more` past that:
  ```
  windows: [w1] Notes (focused)  [w2] Terminal
  w1: [b1] button "New note"  [t2] textbox "Title" = "Groceries"  [l3] list "Notes" 12 items, selected "Groceries"
  ```
- The view is not in the prompt. 0010's State line (at most 64 tokens) carries `app`, `window` and `windows`. `ui_view`'s answer is `Data` under 0002's rule: it cannot start an action, add one or lower a tier. It is read in a second round, inside the 1,024-token answer reserve. Declared app actions (`<app>_<function>`) come first; `ui_view` then `ui_act` is for controls with no declared action. Whether Qwen3-4B does view-then-act reliably is unmeasured; 0010's task set must include it.
- **Takeover.** Any key or click by the user in a window the agent is driving stops the run, and the panel says "Stopped: you took over".
- "Pull up my browser and go to youtube" needs no tree: from v0.2 it is 0006's `browser_open`, at the run tier.

### 7. Trusted path

- `desktop`'s system layer holds 0001's USER right. It draws the panel, every ask card, the file picker, and the agent's 2 px outline and ghost pointer. No app surface can cover that layer, read it, or get keys while it has focus. Only system programs get the `panel` object.
- Title bars are drawn by `desktop`: the kernel's process label, then the app's title. An app cannot pose as another app or as the system (Nitpicker's labels).
- An ask card takes y, n or a click. Enter does nothing, and keys typed before the card appeared are dropped, as `:apply` does (docs/decisions.md:99). `y` mints 0001's `ONCE` handle for the action exactly as shown. A non-ASCII character in an argument shows as a `\u{...}` escape.
- `:apply` keeps its kernel-drawn confirmation (docs/decisions.md:94): the kernel revokes the display `Shm`, draws the card on its console, reads y or n before `desktop` sees input, then hands the screen back.
- Calls go one way. The agent core (`shell` today) calls `desktop` to take the next panel request, to report states and to run its rows. `desktop` calls no service, so routes form no cycle (0001).

### 8. The look: "Ember Workbench"

- A no_std crate `look` holds the tokens, type styles, motion values and `contrast()`. The kernel console uses only its integer colour constants. A CI test fails any text pair under 4.5:1 and any mark or edge under 3:1 (WCAG 2.2 1.4.3, 1.4.11). A lint fails colour literals outside `look` and the kernel console.
- Themes are `kai-theme 1` text files (`token = #RRGGBB`, `style = family size/line weight`, `motion = name ms curve`), parsed and fuzzed in `look`. Saving one re-skins the screen in the next frame. Appearance is one run-tier row with undo, `appearance_set(setting, value)`: theme, a token, scale, text size (100-200%), reduced motion, legible type.

Colour. Dark takes #17's palette (kernel/src/console/theme.rs:9-23) and adds `raised`, `line_strong` and `accent`. Ratios are against `window`, recomputed for this ADR.

| Token | Dark | Light | Job; contrast dark / light |
|---|---|---|---|
| desk | `0D1017` | `E9E4DA` | the desktop |
| window | `141821` | `FAF8F4` | windows, top bar, command line |
| raised | `1B202B` | `FFFFFF` | sheet, menus |
| line | `232A38` | `D9D3C7` | dividers only (decorative) |
| line_strong | `5F6B80` | `857E71` | window frames, inputs: 3.30 / 3.79 |
| text | `ECE6DA` | `1A1D24` | 14.29 / 15.90 |
| muted | `9AA3B4` | `4F5666` | 7.00 / 6.93 |
| dim | `7C8AA8` | `5B6271` | 5.12 (4.70 on raised) / 5.77 |
| ai | `FF9F5A` | `A84A0C` text, `C4560C` marks | the AI and nothing else: 8.76 / 5.42 (marks 4.23) |
| ask | `F5C451` | `7F5800` | waiting for you: 10.91 / 6.00 |
| error | `FF6B6B` | `B42318` | failed or refused: 6.40 / 6.20 |
| ok | `7FD49B` | `26703F` | done, undo available: 9.97 / 5.70 |
| accent | `8CB8FF` | `1F5FBF` | apps and links: 8.81 / 5.74 |
| focus | `ECE6DA` | `1A1D24` | a 2 px ring with a 1 px gap |

A high-contrast theme is `000000` and `FFFFFF`, with ember kept (10.36:1 on black).

Type, in logical px. All three families are SIL OFL-1.1.

| Style | Font, size/line, weight | Used for |
|---|---|---|
| small, body, label | Inter 4.1: 12/16 400; 14/20 400; 13/18 500 | captions; text; buttons and menus |
| heading, display | Inter 18/24 600; 28/34 600 with `opsz` 32 | headings; settings titles |
| mono, mono_title | JetBrains Mono 2.304: 14/20 400 (bold 700); 12/16 500 | terminal, editor, code in asks; window titles, tabs, `kai>`, action lines |
| legible (setting) | Atkinson Hyperlegible Next and Mono, 1 px larger | low vision |

Motion. Nothing the user starts waits behind an animation; only AI surfaces move.

| Token | Value | Used for |
|---|---|---|
| instant | 0 ms | typing, caret, focus, window open, close and raise, menus |
| quick | 90 ms, cubic-bezier(0.2, 0, 0, 1), opacity | hover and press on controls |
| state | 160 ms cross-fade | command-line state changes |
| sheet | spring, 0.28 s, bounce 0 (stiffness 503.6, damping 44.9), translate only | the sheet; interruptible, keeps its velocity |
| settle | 480 ms linear opacity | the agent's outline after an action |
| tick | 125 ms steps | the thinking dots, as #17's status bar |
| reduced | all 0; the dots become a seconds counter | the setting, and every `test=` boot |

No blur, translucency or window-content animation, and nothing longer than 0.5 s.

- **Chrome.** Title bar 32 px, title in `mono_title`. Three 28x28 controls (Lucide `minus`, `square`, `x` at 16 px). Frame 1 px, radius 6 px. Only the focused window has a `line_strong` frame and a shadow (two nine-slice layers); others have a `line` frame and a `dim` title. Resize grabs reach 6 px outside the frame, corners 16x16. While the agent acts on a window, a 2 px `ai` outline sits 2 px outside its frame.
- **Top bar.** 28 px, #17's status bar grown up (kernel/src/console/status.rs:1-8): brand, open windows as mono tabs, model state, CPU and memory, and a clock once the kernel has one.
- **The AI panel** is a full-width 36 px command line at the bottom: `kai>` in bold `ai`, the input, then the state on the right. It takes English or Rhai, as `kai>` does now, and is also the launcher ("notes" opens Notes). Super or Ctrl+Space focuses it from anywhere (whether QEMU and VirtualBox pass both keys to the guest is unverified). A sheet (`raised`, at most 960 px wide and half the screen tall) opens above it only for answers longer than one line, the log and asks. A run-tier request never opens it.

| State | Shows |
|---|---|
| idle | `kai>`, "ask, or type Rhai" in `dim`, the model dot |
| listening | ember block caret, a 2 px `ai` top rule |
| thinking | dots every 125 ms, "thinking 4s", "Esc stops"; shown within 100 ms of Enter |
| acting | the call in mono and its target ("Save (button) in Notes"); the outline on that window |
| done | an `ok` check, code's result line ("Done: X (was Y). undo"), `undo` and `show` |
| asking | the sheet with a 3 px `ask` bar, each action and its arguments in mono, the reason, a 2:00 countdown (shellcore/src/actions.rs:19), Run (y) and Don't (n) |
| refused | an `error` bar and the reason |

The panel reads its states from 0002's structured `Outcome` and `actlog 1` entries, never from reply text.

### 9. Scale

Every size is in logical px. The scale comes from the EDID physical size when there is one, otherwise from the width (up to 1920 wide 1.0, up to 2560 1.25, up to 3840 2.0), in quarter steps from 1.0 to 3.0; a setting overrides it. At those defaults 1080p, 1440p and 4K give 1920x1080, 2048x1152 and 1920x1080 logical px, so layouts barely change. Native apps render at the exact scale. Foreign and bridged surfaces get an integer scale and are resampled.

### 10. Linux apps (P6)

- In v0.2 the guest's `cage` draws on the guest's virtio-gpu, and `vmm` copies each damaged rectangle into one `Surface` (XRGB8888), shown as one window (0006). `desktop` draws its frame and title. The agent gets the window rows and 0006's guest rows, not nodes. Input for that window goes from its event ring to the guest's virtio-input.
- Per-window integration comes later through a guest-side bridge in the style of Sommelier, over virtio-gpu's cross-domain context. Because `kai-surface 1` keeps Wayland's objects, each guest toplevel maps 1:1 to a toplevel here.
- `wlbridge`, a static Linux binary on wayland-server 0.31.14, wayland-backend 0.3.17 and Smithay 0.7.0 that forces server-side decorations, is built only if 0006 takes its `lxabi` fallback.

## Consequences

- One protocol for native apps. Linux display is a translation, never a redesign.
- The agent sees every native app as typed nodes and declared actions, with no screenshots and no vision model.
- Kernel work comes before the first window: 0001's `Shm` with user unmap and shootdown, `Display`, `Input` and the input queue. Designs A and C sized it too low.
- We own a toolkit and a UI interface that must be versioned like the syscall ABI.
- CPU only through v1.0 (0007): no blur or glass, 4K may animate at 30 Hz, and VirtualBox at 16 MiB of VRAM stops at 1440p.
- The compositor joins the trusted base for asks; `:apply` does not.
- The fonts add about 1.2 MB to the boot image (Inter 879,708 B and JetBrains Mono 300,144 B, D's figures, not re-checked) until P2 gives files.
- A desktop of apps needs more than 16 processes: `desktop`, one `comphost` per app, `scriptrun`. 0003 raises `MAX_PROCESSES` to 64.
- AccessKit (0.x), skrifa (about monthly) and harfrust (0.x) move fast. They are pinned and upgraded on purpose.
- Linux apps stay opaque to the agent below window level until a guest bridge carries their trees.

Effort for v0.1, in lines including tests (estimates):

| Step | Lines |
|---|---|
| `look` with #17's colours and the contrast test | 300 |
| Kernel: `Display`, `Input`, the input queue, the console hand-over (0001's `Shm` work not counted) | 800 |
| `surfabi`: objects, header, codec, fuzz target | 900 |
| `gfx2d`: raster, fonts, atlas, box drawing, golden images | 2,100 |
| `deskcore` and `desktop`: scene, damage, focus, chrome, cursor, screendump cases | 2,500 |
| Panel, sheet, states, agent view and rows | 2,500 |
| `uikit` and `kai:ui` in `comphost`, with AccessKit | 3,500 |
| The terminal as the first client | 1,200 |
| Motion, scale, light, high-contrast and legible themes, `kai-theme 1`, `appearance_set` | 1,200 |
| Total | about 15,000, in about 17 PRs |

## What it costs to change later

| Change | Cost | Why |
|---|---|---|
| tiny-skia to vello_cpu or a GPU renderer | Low | Behind `gfx2d` and `desktop`; clients do not see it |
| Colours, fonts, sizes, motion | Low | Tokens and data; the lint keeps it so |
| GPU buffers | Medium, additive | A `kai-surface 2` buffer type; version 1 clients keep working |
| Display or input drivers into ring 3 | Low | `desktop` talks to handles, not devices |
| Wayland as the native protocol | Medium | The objects match. `uikit`'s transport and `desktop`'s front end change; apps do not. |
| Server-drawn UI (Haiku, Flatland) | High | `uikit` moves into the compositor; trees and actions survive |
| Replacing AccessKit | Medium | `surfabi` mirrors a pinned subset |
| Moving the AI panel | Moderate | Apps are told their work area, and users learn where it is |
| Logical px and the scale contract; the reserved AI colour and trusted layer; labels as names | High | Apps depend on them; missing accessible names are the classic retrofit |
| `kai:ui` | High once frozen | 0003's freeze rule; every app uses it |
| `kai-theme 1` | Moderate | Users will edit theme files, so it is versioned from its first release |

## Spikes that settle it

**S05-1 "compose and look"** joins A's, C's and D's spikes, on a throwaway branch.
- **Build.** Two spike-only kernel calls behind the kernel's `spikes` feature (kernel/Cargo.toml:15; `--spikes`, xtask/src/main.rs:3): one maps virtio-gpu's back buffer in 4 KiB pages and flushes a list of rectangles; one shares a buffer between two processes with a cross-process wake. `spikes/compose` is a hard-float ring-3 program on `gfx2d`'s stack, with Inter and JetBrains Mono as boot modules. Scenes (a) and (b) also run on vello_cpu 0.3.0.
- **Scenes,** 300 frames each: (a) a full composite with wallpaper and four windows, one with alpha; (b) an 800x600 window dragged 200 px with its shadow; (c) a 160x45 page of terminal text, with a cold and a warm atlas; (d) the ask sheet's spring, 17 frames; (e) key to flush: one process redraws a 600x400 region per injected key and the other composites it; (f) scene (b) with one of M10's CPU hogs (`work`, user/src/bin/work.rs:6) on every CPU under `classes`. Plus screendumps of every panel state in the dark and light themes.
- **Measure.** Render and flush separately with the TSC; p50, p95 and p99; at 1280x800, at 1920x1080 at scale 1.0, at 2560x1440 at 1.25 and at 3840x2160 at 2.0 (`display=`). Also the boot image's growth from the fonts.
- **Where.** QEMU with KVM on GitHub Actions' ubuntu-24.04 runners, gated; CI already boots cases under KVM and TCG (.github/workflows/ci.yml:44-69). TCG locally in WSL2 with QEMU 10.2.1, reported only, plus a check that boot cases stay stable. The founder may look at the screendumps, and at the scene on GOP in VirtualBox at 1280x800. No model runs anywhere; "thinking" is simulated.

| Scene, KVM, 1920x1080 | Pass |
|---|---|
| (a) full composite | at most 8 ms |
| (b) drag | p99 at most 8 ms render plus 4 ms flush |
| (c) text page | warm at most 4 ms, cold at most 30 ms |
| (d) sheet spring | p95 at most 4 ms |
| (e) key to flush | p99 at most 8 ms |
| (f) drag under load | p99 at most 16.7 ms |

At 2560x1440 every figure must be within 1.8x of these. At 3840x2160, (a) must take at most 16 ms, or 4K animates at 30 Hz in v0.1. Under TCG at 1280x800, (b) p99 must be at most 50 ms.

Fail rules, set in advance:
- (b) over 16.7 ms at 1080p after SSE2/AVX2 blits and banding across worker threads: switch to vello_cpu if it passed, else re-judge with a GPU or virgl path brought earlier. The protocol and the agent view do not change.
- (e) over 16 ms: spin 1 ms on `commit_seq` before `NOTIFY_WAIT`. Still over: re-judge with compositing in the kernel.
- (f) over 16.7 ms while (b) passes: give `desktop` a frame deadline in `classes`, as a scheduler change with its own decision row.
- (d) over 4 ms: drop the sheet's opacity and keep the translate; over 8 ms, states change instantly. (b) over 8 ms: no shadow while dragging.
- Text judged poor on the screendumps: tune hinting and the coverage table first, then try swash on skrifa 0.44. That adds a second font parser, so it comes back for a decision.

**S05-2 "view size"** (host test in CI). Mock trees for each P4 app (terminal, files, notes, settings, system monitor, model manager, editor), rendered as the agent view and counted with `bpe` and the GGUF vocabulary. Pass: each at most 300 tokens. Fail: change the projection (drop static text, shorten roles) before the view's format freezes.

**Only if 0006 takes its `lxabi` fallback:** B's census. Build a static musl Smithay bridge with two shm clients on an Ubuntu runner, run it under `strace -f -c`, and record the distinct calls, resident memory and binary size. It sizes `wlbridge`'s needs from the Linux ABI.

## Phases that depend on it

The order of work is in [0000](0000-execution-plan.md).

| Phase | Needs from this ADR |
|---|---|
| P1 | `look` with #17's colours and the contrast test; 0002's `Outcome` and `actlog 1`, which the panel will show; a status line within 100 ms of Enter in today's terminal |
| P2 | Fonts, themes and keymaps move from boot modules to files ([ADR 0004](0004-storage.md)); `kai-theme 1` files become editable |
| P3 | All of it: `Display`, `Input`, `Shm` (0001), `surfabi`, `gfx2d`, `desktop`, the panel, `uikit`, the agent's rows. Exit: boot to a desktop; windows open, move and close; the agent opens apps and acts inside them (roadmap.md:142). |
| P4 | Every app on `uikit` with declared actions; the terminal app with typed spans; light, high-contrast and legible themes; `appearance_set` |
| P5 | Network asks use the ask card; fonts and apps as packages ([ADR 0011](0011-packages.md)) |
| P6 | The guest screen as a foreign surface (v0.2); a per-window guest bridge after it; `wlbridge` only on the `lxabi` fallback |
| P7 | GOP on the founder's PC: honour the colour masks (kernel/src/console/mod.rs:117-119); scale from a real monitor's EDID; input from `usbd` (0007) |
| P8 | Fuzzing of `surfabi`, the tree decoder and the `kai-theme 1` parser; a screen reader on the same trees; a security review of the trusted path |

## Changes to recently merged and open work

- **#17 Ember terminal (merged):**
  - Follow-up PR now: a new no_std crate `look` holding #17's colours (kernel/src/console/theme.rs:9-30) and `contrast()`, with a host test for the 4.5:1 claim that is only a comment today (theme.rs:4). theme.rs takes its colours from `look`; nothing on screen changes. Why: the desktop needs the same tokens, and the claim should be checked.
  - Follow-up PR now: fix the pointer's cell mapping. `Cursor::cell` divides from pixel (0,0) by the font height (termcore/src/mouse.rs:156-165; kernel/src/console/mod.rs:106-111, :148-149), but #17 moved the text area to `left` and `top` with a 9/8 row pitch (kernel/src/console/layout.rs:71-76, :95-112). At 1280x800 the `pointer` case's pixel (38,40) is drawn in text row 0, column 2 (top 34, pitch 18, left 24; arithmetic, not run), while the pointer reports, and the case expects, row 2, column 5 (kernel/src/pointer.rs:76-81; xtask/src/cases.rs:125). `cell` should take the origin and pitch, and the case should move.
  - At P3 start: the rest of the token table goes into `look`. Row colour by text (`Kind::of`, termcore/src/style.rs:58-80) stays in the kernel console only; the P4 terminal gets typed spans. The Noto bitmaps stay in the kernel for the recovery console (0008).
- **#18 virtio-gpu (open, draft):**
  - Before merge: rebase onto main. It still edits kernel/src/console.rs, which #17 replaced with kernel/src/console/; `git merge-tree` against `ec5ecff` reports a modify/delete conflict. After the rebase, the console draws into the back buffer and calls `display::flush(rect)` after every direct write: rows, the busy sign and the 125 ms status bar (kernel/src/console/status.rs:21). Otherwise nothing #17 draws shows on virtio-gpu. Rename `display::Surface` (branch kernel/src/display/mod.rs:27) to `display::BackBuffer`, as 0007 asks, since `Surface` is this ADR's.
  - P3 follow-up: `flush` takes up to 32 rectangles (it takes one, branch display/mod.rs:141); the console stops painting while `desktop` holds the screen; the back buffer is handed out as a kernel-owned `Shm` in 4 KiB pages, not with `link_window`; the cursor calls are reached through `Display`. 0006 separately moves the wire layouts into `gfxcore`.
- **#19 USB input (open, draft):**
  - No driver change. The HID GUI keys already map to set 1 `E0 5B` and `E0 5C` (branch xhci/src/hid.rs:99-110). Keep tablets absolute end to end.
  - Follow-up PR on main, any time: a Super token in `termcore::keys`, which has none (termcore/src/keys.rs), so Super can open the panel.
  - In P3, `pointer::Event` (branch kernel/src/pointer.rs:134-197) and the keys become `InputEvent`s in the `Input` queue. After S07-1, `usbd` feeds the same queue (0007).
- **#15 seamless actions (merged):** the panel needs states, but `Reply` carries only `ok`, `text` and `confirm` (shellcore/src/lib.rs:120-126). The structured outcome the judge asked for is 0002's `Outcome`, so there is no separate PR on `Reply`. The `ui_*` and `window_close` rows join 0002's registry in P3, from which `TIERS` (shellcore/src/tiers.rs:35-44) is generated.
- **#13 parser hardening (merged):** in P3, add hostile tests (testkit/hostile.rs) for `surfabi`'s decoder, the tree decoder and the `kai-theme 1` parser.
- **#14 faster decode (merged):** none.
- **#16 networking (open):** none.
- **Other ADRs:** 0001's kind table gains `Display` and `Input`; 0002's P3 row list swaps `ui_close` for the shared `window_close`.

## Open questions for the founder

1. **The look.** After S05-1's screendumps, do you approve dark-first "Ember Workbench", with the bottom `kai>` line as the AI panel and launcher? Or do you prefer light-first "Daylight Document": flat pages, Atkinson type, an AI column on the right?
2. **Decided 2026-10-22:** the OFL fonts are kept and Unicode-3.0 is in the free class, so Inter, JetBrains Mono and Atkinson Hyperlegible Next ship under OFL-1.1, and the font crates' `yoke` and `zerofrom` are free. (The answer was "keep ... the OFL fonts"; read here as covering these three. Say if it was meant for Noto only.) C libxkbcommon's HPND files would still come back to you, only if ADR 0006 falls back to `lxabi`.
3. **Linux apps and the agent in v0.2.** The agent can open, focus and close the Linux window and use 0006's browser rows, but cannot press buttons inside Linux apps. That needs accessibility inside the guest carried out by the bridge, or synthetic input at the ask tier. Is window-level control plus the browser rows enough for v0.2?
4. **CPU-only graphics.** GOP on the RTX 4070 Ti through v1.0 (0007's question 4) means no blur or glass, 4K that may animate at 30 Hz, and 1440p at most in VirtualBox. Is that acceptable, or should GPU work move earlier, at a cost of years?
5. **Who draws questions.** Ask cards are drawn by the ring-3 compositor, not the kernel; `:apply` stays kernel-drawn. Does that meet your bar for "an app cannot fake a yes"?

## Sources

Repo, main at `ec5ecff` unless a branch is named: kabi/src/call.rs:6, :108-110; kabi/src/lib.rs:82-108, :160; kabi/src/model.rs:65; kernel/src/memory/user.rs:5-20, :183; kernel/src/process/mod.rs:54; kernel/src/console/mod.rs:1-12, :106-111, :117-119, :148-149; kernel/src/console/fb.rs:1-7; kernel/src/console/layout.rs:71-76, :95-112; kernel/src/console/status.rs:1-8, :21; kernel/src/console/theme.rs:1, :4, :9-30; kernel/src/pointer.rs:76-81; kernel/Cargo.toml:15; termcore/src/mouse.rs:156-165; termcore/src/style.rs:58-80; termcore/src/keys.rs; shellcore/src/lib.rs:120-126; shellcore/src/tiers.rs:35-44; shellcore/src/actions.rs:19; livecore/src/settings.rs:38; xtask/src/userbuild.rs:24; xtask/src/vbox/mod.rs:330-331; xtask/src/cases.rs:125; xtask/src/main.rs:3; user/src/bin/work.rs:6; Cargo.toml:14-18; .github/workflows/ci.yml:44-69; testkit/hostile.rs; docs/decisions.md:39, :74, :94, :99, :138, :140, :177, :203, :205; docs/roadmap.md:131, :139, :142; docs/research/live-editable-system.md:54, :102; docs/third-party-notices.md:6-17; docs/images/terminal-after.png. Branches: origin/dane/virtio-gpu-a5daaa (kernel/src/display/mod.rs:1-12, :27, :141-179; docs/decisions.md:197), origin/dane/usb-input-83eb7c (xhci/src/hid.rs:99-110; kernel/src/pointer.rs:134-197; kernel/src/usb.rs:1-5). P0 work files: D:/kai-targets/p0/work/decision-sheet.md and 05/ (judgement, designs A-D, research-code, research-prior).

External, read 2026-10-21 unless dated:
- Wayland protocol book: https://wayland.freedesktop.org/docs/book/Protocol.html. wayland.xml on main, lines 321 and 532-536: https://gitlab.freedesktop.org/wayland/wayland/-/blob/main/protocol/wayland.xml. `WL_MAX_MESSAGE_SIZE` 4096 at src/wayland-util.h:99 in the same repository. wayland-protocols 1.49 (2026-06-07), for xdg-shell, xdg-activation-v1, fractional-scale-v1 and cursor-shape-v1: https://gitlab.freedesktop.org/wayland/wayland-protocols.
- crates.io (https://crates.io/crates/NAME): tiny-skia 0.12.0 (2026-02-02), skrifa 0.48.0 (2026-10-03), read-fonts 0.45.1 (2026-10-15), harfrust 0.14.0 (2026-10-04), accesskit 0.25.1 and accesskit_consumer 0.39.1 (2026-09-25), vello_cpu 0.3.0 (2026-10-02), swash 0.2.10 (2026-07-17), parley 0.12.0 (2026-10-09), smithay 0.7.0 (2025-06-24), wayland-server 0.31.14 (2026-07-22), wayland-backend 0.3.17 (2026-08-14), resvg 0.48.1 (2026-08-02). Dependency pages: read-fonts 0.45.0 and 0.45.1 require `yoke ^0.8.0` (feature `derive`); yoke 0.8.x, zerofrom, yoke-derive and zerofrom-derive are Unicode-3.0; skrifa 0.49.0 and read-fonts 0.46.0 (both 2026-10-21) still require `yoke`, and harfrust 0.14.0 requires read-fonts ^0.45.0; the fallback skrifa 0.46.0 (2026-08-12) and harfrust 0.13.0 (2026-08-13) take read-fonts ^0.43, and read-fonts 0.43.3 (2026-08-25) has no `yoke`. swash 0.2.10 requires skrifa `>=0.31.1, <=0.44`; parley_engine 0.12.0 requires skrifa ^0.44.0, harfrust ^0.12.0 and icu_* 2.3 crates, none optional; smithay 0.7.0 requires xkbcommon ^0.8.0, not optional.
- Redox, "Smallvil performance on Redox is not adequate": https://www.redox-os.org/news/this-month-251130/ (2025-11-30).
- Chrome 145 removed FreeType from Blink: https://developer.chrome.com/blog/memory-safety-fonts (published 2025-03-19; the note saying so is dated 2026-04-24).
- OSWorld, the a11y-tree length analysis (about 6,000 tokens of context for about 90% of single observations): https://arxiv.org/html/2404.07972.
- AccessKit (both crates `#![no_std]`, read on docs.rs): https://github.com/AccessKit/accesskit. Weston 16.0.1 (tag 2026-10-19): https://gitlab.freedesktop.org/wayland/weston. Orbital: https://gitlab.redox-os.org/redox-os/orbital. HaiWay: https://github.com/X547/wayland-server. Sommelier and cross-domain: https://crosvm.dev/book/devices/wayland.html. Flatland: https://fuchsia.googlesource.com/fuchsia/+show/main/docs/concepts/ui/scenic/flatland/index.md. Nitpicker: https://acsac.org/2005/abstracts/54.html. rio(4): https://plan9.io/sources/plan9/sys/man/4/rio.
- libxkbcommon LICENSE: https://github.com/xkbcommon/libxkbcommon/blob/master/LICENSE. Slint 1.18.1 (2026-09-21) LICENSE.md: https://github.com/slint-ui/slint.
- WCAG 2.2, Recommendation of 2024-12-12: https://www.w3.org/TR/WCAG22/. Apple's spring parameters (2023): https://developer.apple.com/videos/play/wwdc2023/10158/.
- Fonts, all OFL-1.1: Inter 4.1 (2024-11-16) https://github.com/rsms/inter; JetBrains Mono 2.304 (2023-01-14) https://github.com/JetBrains/JetBrainsMono; Atkinson Hyperlegible Next (no release; pushed 2025-06-10) https://github.com/googlefonts/atkinson-hyperlegible-next.
- Alacritty 0.17.0 (2026-04-06), `license = "Apache-2.0"` at alacritty/Cargo.toml:5: https://github.com/alacritty/alacritty. Lucide 1.61.0 (2026-10-21), LICENSE (ISC, with the Feather-derived icons listed under MIT): https://github.com/lucide-icons/lucide.
- USB HID Usage Tables: https://www.usb.org/hid (the edition is pinned in the PR that writes the table).
