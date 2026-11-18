# ADR 0005: Graphics: compositor, surface protocol and visual direction

Status: Amended 2026-11-14: Kai Desk, Implemented 2026-11-15. The look, the AI's place on the screen and the order of the trusted layer changed (see "Amendment 2026-11-14" below). The founder approved the amendment with its four calls at their defaults on 2026-11-15 (#925), and Kai Desk PRs 5-10 built it (#935, #940, #943, #944, #947 and the finishing PR). Accepted 2026-10-22 (proposed 2026-10-21). The founder approved it with their answers of 2026-10-22 folded in; those points are marked "decided 2026-10-22". Questions still open below keep this ADR's stated default until the founder answers.

Repo citations are `path:line` on main at `ec5ecff`. Open work is cited at its branch tip: #18 virtio-gpu (`origin/dane/virtio-gpu-a5daaa`, `7603e54` plus merge `af7635d`), #19 USB input (`origin/dane/usb-input-83eb7c`, `0717caa`) and #16 networking (`origin/dane/net-dhcp-dns-b5bdb7`). Nothing was built or run for this ADR, and no model ran anywhere. Crate versions, licences, features and dependencies were re-read on crates.io on 2026-10-21 from the founder's PC (web reads only), as were the repo citations against `ec5ecff`; contrast ratios are WCAG 2.2 arithmetic.

## Amendment 2026-11-14: Kai Desk

The founder asked for a desktop that looks and works like Windows 11, with a Kai twist ("Start redesign now"). This amendment says what that means. It replaces the "Ember Workbench" look and the `kai>` bar along the bottom of the screen, and keeps every rule of the trusted path. Where the text below says otherwise, this section and the amended §1, §7, §8 and §9 win.

**In plain words, what a person sees:**

- The top bar and the `kai>` bar along the bottom are gone. A **taskbar** runs along the bottom instead: a Kai button and your apps' icons in the middle, and at the right a Kai chip (ready, thinking, done), the memory in use and the time.
- Kai has a **pane** down the right edge: your lines, Kai's answers, the state line and the input. It is closed when the desktop starts. Ctrl+Space, or a click on the corner of the screen, opens it; so does a click on the chip.
- A **Kai menu** (Start in the code) opens with the Super key, Ctrl+Esc or the Kai button. It has one box that says "Ask Kai or search apps", your pinned apps, your own recent lines and all apps. Type "notes" and press Enter and Notes opens; type anything that is not an app and press Enter and it asks Kai.
- Windows get rounder corners, wide flat minimize, maximize and close buttons, and titles that show the system's name for the app first and the app's own title after it. This is the rule ADR 0005 §7 always had; the code did not follow it.
- The look is graphite in layers (desk, shell, window, raised), with ember kept for Kai alone. The shell and hover colours are new and desktop-only; apps never get them. Light and high-contrast follow.
- Nothing about safety changes. The ask card is drawn last, over everything, takes only y, n or a click, and Kai's words never appear in the system's look or in the state line.

**Four calls the founder is asked to confirm** (each is the default this amendment builds; the OK is needed before the first visible code, the window chrome, merges):

1. **The pane floats and is closed at boot.** It sits over windows rather than pushing them aside; only a maximized window is made to fit beside it. Docking it is open question 9.
2. **The clock shows UTC**, with "UTC" written on its date line, until the person sets a fixed offset (built 2026-11-18, open question 7).
3. **The fonts stay Inter and JetBrains Mono.** Both are free to use under the OFL and neither is Microsoft's. A typeface drawn for Kai is a separate, large job. Open question 6.
4. **Super opens the menu** and Ctrl+Space the pane. Whether Super reaches the guest in VirtualBox and QEMU is unchecked; the Kai button, the `apps` line, Ctrl+Space and the corner always work. Open question 1.

**Implemented (2026-11-15).** The window chrome (#935), the Kai pane (#940), the taskbar, tray and UTC clock (#943), Start with its pins and tooltips (#944) are on main; the mouse wheel over the pane and Start (#947) and the finish (#951: tour shots at 1280x800, 1024x768 and 1920x1080, desk-perf scene (g), the founder walk closing the pane by its close item before it clicks, and the card-pixel test with a taskbar hover) land with #947 and #951. What stays open: whether Super and Ctrl+Esc reach the guest in VirtualBox and QEMU and get past the Terminal's keyboard lease (open question 1); a typeface of our own, a time-zone setting and the wheel for apps, each at its default (open questions 6-8); docking the pane (open question 9); and §9's `scale::for_screen`, which the desktop did not call then (wired 2026-11-18, §9). desk-perf at 1920x1080 under KVM stayed within every S05-1 budget; (a)-(f) are from #944's head f344e244 (run 43751047213), (g) from #951's run 43770528735:

| Scene | p99 (max where judged) | Budget |
|---|---|---|
| (a) full composite | 4.47 ms | p99 8 ms |
| (b) drag render / flush | 1.29 / 0.75 ms | p99 8 / 4 ms |
| (c) 160x45 text page warm / cold | max 1.69 / 4.75 ms | max 4 / 30 ms |
| (e) key to flush | 6.12 ms | p99 8 ms |
| (f) drag under load | 11.79 ms | p99 16.7 ms |
| (g) full composite with Start and the pane open (#951's run 43770528735) | 6.19 ms | p99 8 ms |

**What changes in this document:** §1 and the Decision paragraph (what `desktop` draws), §7 (additions to the trusted path), §8 (renamed "The look: Kai Desk", with the new tokens, metrics and pixel tables), a note in §9, one row of the cost table, the open questions, and two rows of the options and reuse tables. The decision row is `2026-11-14-kai-desk`. It supersedes only the placement half of `2026-11-14-panel-history` (the history moves from the desk into the pane); that row's look rules stay. `2026-11-14-desktop-lines-ask-on-the-card` is unchanged. The row at decisions.md:229 is not edited.

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
| D. Visual direction (8) | "Ember Workbench" (dark-first, Ember tokens, Inter and JetBrains Mono, a bottom `kai>` line as the AI panel) against "Daylight Document" (light-first, Atkinson type, an AI column on the right) | Continues the Ember look the founder has seen (docs/images/terminal-after.png). Every contrast figure checks. Contrast becomes a CI test. | Text quality is a guess until screendumps exist. It named swash on skrifa 0.48, which swash 0.2.10 does not allow. | **The look**: Ember Workbench, with Daylight's Atkinson pair as the legible setting. Amended 2026-11-14: Kai Desk (§8); the Atkinson pair stays |

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
| Icons | Lucide 1.61.0 (ISC, free; icons from Feather, `minus`, `square` and `x` among them, are MIT), rasterised at build time by resvg 0.48.1 (Apache-2.0 OR MIT) in xtask. Superseded 2026-11-14: the Kai glyphs of §8, authored fresh and drawn in code, with nothing traced from Lucide, Fluent or Segoe | no other icon set was weighed |

## Decision

Kernel AI gets its own small window system. One program, `desktop`, draws every window, the Kai taskbar, the Kai menu (Start) and the Kai pane, and every question the AI asks, so no app can fake a question or a "yes". Apps describe their buttons and fields as well as drawing them, so the agent can read and press them without screenshots. The look is Kai Desk (amended 2026-11-14): a Windows 11-style desktop in the dark "Ember" colours you have seen, and Linux apps arrive in v0.2 as one window drawn by a small Linux VM.

In the options' terms: base design A; the look is design D's "Ember Workbench", renamed and rebuilt as Kai Desk on 2026-11-14; grafts are C's text view, takeover rule and per-surface header page, and B's connection given at spawn. Names follow the decision sheet: the judge's `surface@1` is `kai-surface 1`, and the judge's `ui.*` rows are snake_case.

### 1. Shape

- A ring-3 compositor, the program and service `desktop`, owns the scene, damage, focus, window decorations, the cursor, the Kai taskbar, menu (Start) and pane, and the agent's view. Its logic is in a host-tested crate, `deskcore`. It builds for the hard-float target and runs under the supervisor. Under the `classes` policy it is never labelled `background`, so background threads never preempt it on wake, though one already running keeps a 2 ms shield (docs/decisions.md:39, :177). `classes` has no frame deadline; S05-1's scene (f) shows whether one is needed.
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
| toplevel | set_title, set_app_id, set_min_size, set_max_size, ack_configure, set_unsaved(bool), set_agent_private(bool) | configure(serial, w, h, states), close | `xdg_toplevel` |
| popup | position(anchor, gravity, offset), grab | configure, done | `xdg_popup` |
| seat | set_cursor(shape name) | focus_in, focus_out, modifiers | `wl_seat`, cursor-shape-v1 |
| a11y | tree_update(bytes), commit, bind_action(node, action name; empty unbinds) | action_request(node, action, value) | ours |
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

- **A private window** (`toplevel.set_agent_private`, `kai:ui` 0.1.1 `set-agent-private`) is kept from the agent: `ui_view` gives its title and "private" with no nodes, and `ui_act` on it is refused, focus included. The user sees and uses it as any window. Only an app the system image grants it (Settings, whose Rules and Access pages hold typed-only data) may ask, and wasmhost traps the call for any other; the flag reaches `desktop` before the window's tree.
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
- An ask card takes y, n or a click. Enter does nothing, and keys typed before the card appeared are dropped, as `:apply` does (docs/decisions.md:99). `y` mints 0001's `ONCE` handle for the action exactly as shown. A non-ASCII character in an argument shows as a `\u{...}` escape. On the desktop a card takes nothing for the menu's settle time after it shows, and `y` takes two fresh presses, the first arming it; a click on Run takes one (row `2026-11-14-desktop-lines-ask-on-the-card`).
- `:apply` keeps its kernel-drawn confirmation (docs/decisions.md:94): the kernel revokes the display `Shm`, draws the card on its console, reads y or n before `desktop` sees input, then hands the screen back.
- Calls go one way. The agent core (`shell` today) calls `desktop` to take the next panel request, to report states and to run its rows. `desktop` calls no service, so routes form no cycle (0001).

**Amended 2026-11-14 (Kai Desk).** Every rule above stays: the settle time of 400 ms, `y` arming and a fresh `y` running, a Run click needing a fresh pointer entry after the card is drawn, held keys dropped, Enter ignored, `n` or Don't declining at once, a non-ASCII character shown as `\u{...}`, the 2:00 countdown, and `:apply` on the console. Added:

- Paint order: desk, windows, the agent's outline, the taskbar and tray, the pane, the menu (Start), a tooltip, the card, the cursor.
- Hit order: the card, the menu, the pane, the taskbar and tray, windows, the desk. The card takes all input while it is up.
- The menu is painted below the card and also closes on `Focus::Ask`.
- The card keeps its maximum width of 960. Its height grows up to the work height, H-72, and never cuts a call: a call too long to show whole even then is refused with a code-only "too long to show on the card" result, never drawn in part.
- The Kai button carries an `ask` ring while a card is up. `ask` is used nowhere else.
- The state line is code-only in both the pane's status row and the tray chip.
- The AI's words appear only in the pane's history rectangle, in the AI look.
- Title bars and tooltips show the kernel's label before the app's title. This restores the rule above; the code before this amendment showed the title alone.
- The taskbar draws no app text outside label-first tooltips.
- Menu rows are data: a catalog row is never queued to the model, and only text the person typed is sent.

What this costs, said plainly. The AI's words move from a layer under the windows to a system pane drawn over them, beside the card's region. That weakens the story that the system's surfaces speak only code, so the rule is pinned and tested rather than assumed. The words show only in the history rectangle, clipped so they never reach the status row, the input, the taskbar or the card. They are drawn in the AI's look (a `Kai` label in `ai`, a 2 px `ai` rule, sans `text`), never in the card's header style, mono call lines or the `ask` colour. `y` does nothing without a card, so "Kai asks to run ... press y" inside a model's text is only text. The tests: a hostile `said` never changes the status row's or the chip's pixels; no `ask`-coloured pixel and no card-header style appears in the pane while no card is up; and the card's pixels are identical with the menu, the pane, a tooltip and a taskbar hover active, at all three sizes.

### 8. The look: Kai Desk

Amended 2026-11-14. The name "Ember Workbench" is retired. Its tokens, themes and rules stay where this section does not change them.

- A no_std crate `look` holds the tokens, type styles, metrics (`look::metric`), motion values and `contrast()`. The kernel console uses only its integer colour constants. A CI test fails any text pair under 4.5:1 and any mark or edge under 3:1 (WCAG 2.2 1.4.3, 1.4.11) on every full ground, and holds `hover`, a restricted ground, to its own lists below. A lint fails colour literals outside `look` and the kernel console.
- Themes are `kai-theme 1` text files (`token = #RRGGBB`, `style = family size/line weight`, `motion = name ms curve`), parsed and fuzzed in `look`. Saving one re-skins the screen in the next frame. Appearance is one run-tier row with undo, `appearance_set(setting, value)`: theme, a token, scale, text size (100-200%), reduced motion, legible type. `shell` and `hover` are tokens of a file like the others; a file written before they existed leaves them out and takes its base theme's.

**What it is.** A Windows 11-style desktop built from code we own. It has solid graphite layers (desk, then shell, then window, then raised), radius, spacing and a soft shadow, and no Mica, acrylic, blur or translucency. Ember (`ai`) is the only chrome colour, and it belongs to Kai: the Kai mark, the listening rule and caret, `kai>`, Kai's history entries, the unread badge and the thinking pill. Selection and focus use `accent`. There are no Microsoft fonts, icons or marks, and the UI never says "Start" or "Copilot".

Colour. All 15 earlier values are unchanged, so no earlier golden moves for colour; `shell` and `hover` are new. Ratios are against `window`, dark / light.

| Token | Dark | Light | High contrast | Job |
|---|---|---|---|---|
| desk | `0D1017` | `E9E4DA` | `000000` | the wallpaper, flat |
| shell | `111520` | `F3F0EA` | `000000` | the taskbar, the Kai pane and the menu's footer; desktop only; a full ground |
| window | `141821` | `FAF8F4` | `000000` | windows and input fields |
| raised | `1B202B` | `FFFFFF` | `000000` | the menu, tooltips and the ask card |
| hover | `252B37` | `E4DFD5` | `000000` | hover and pressed fills; desktop only; a restricted ground |
| line | `232A38` | `D9D3C7` | `FFFFFF` | rules and the unfocused frame (decorative) |
| line_strong | `5F6B80` | `857E71` | `FFFFFF` | window frames and field edges: 3.30 / 3.79 |
| text | `ECE6DA` | `1A1D24` | `FFFFFF` | 14.29 / 15.90 |
| muted | `9AA3B4` | `4F5666` | `FFFFFF` | 7.00 / 6.93 |
| dim | `7C8AA8` | `5B6271` | `FFFFFF` | 5.12 (4.70 on raised) / 5.77 |
| ai | `FF9F5A` | `A84A0C` | `FF9F5A` | Kai's text, and Kai's alone: 8.76 / 5.42 |
| ai_mark | `FF9F5A` | `C4560C` | `FF9F5A` | Kai's marks: the outline, caret, rules and mark: 8.76 / 4.23 |
| ask | `F5C451` | `7F5800` | `FFFFFF` | the trusted layer only, the card's bar and the Kai button's ring: 10.91 / 6.00 |
| error | `FF6B6B` | `B42318` | `FFFFFF` | failed or refused: 6.40 / 6.20 |
| ok | `7FD49B` | `26703F` | `FFFFFF` | done, undo available: 9.97 / 5.70 |
| accent | `8CB8FF` | `1F5FBF` | `FFFFFF` | apps, links and selection: 8.81 / 5.74 |
| focus | `ECE6DA` | `1A1D24` | `FFFFFF` | a 2 px ring with a 1 px gap: 14.29 / 15.90 |

`look` has these 17 tokens, and a test pins this table's values to the code. Apps get 12 of them (`Token::APP`): never `ai`, `ai_mark`, `ask`, `shell` or `hover`.

- **`shell` is a full ground.** It joins `desk`, `window` and `raised`, so every text token holds 4.5:1 and every mark 3:1 on it. The worst cases are dark `dim` 5.26 and `line_strong` 3.39, and light `ai` 5.06 and `line_strong` 3.54.
- **`hover` is a restricted ground.** Text held to 4.5:1 on it is `text`, `muted`, `accent`, `error` and `ok`: the dark worst is `error` 5.12, the light worst `ok` 4.56 and `accent` 4.59. Marks held to 3:1 are `focus` and `ai_mark` (light `ai_mark` 3.38). `dim`, `ai` and `line_strong` are never drawn on `hover`, and a painter test enforces it: on `hover` `dim` is 4.09:1 in dark, `ai` is 4.33:1 in light and `line_strong` is 2.64:1 in dark. That is why Kai's colour is `ai_mark`, never `ai`, wherever a `hover` fill can sit behind it: the Kai button while hovered, the chip's unread badge and the Kai mark in the menu's "Ask Kai" row. A hover fill steps 1.28:1 (dark) and 1.17:1 (light) from `shell`.
- **High contrast** is `000000` and `FFFFFF`, with ember kept (10.36:1 on black). Its `hover` equals the ground it sits on, so a painter that finds `hover` equal to its ground draws a 1 px `focus` frame instead of a fill. Hover and selection stay visible, and the painter never reads the theme's name.
- **A colour that clears `window` but falls short on `hover` or `shell` is now refused**, and the message names the pair: light `muted #666666` is 4.32:1 on `hover` and dark `text #888898` is 4.07:1, both fine on every earlier ground. A `kai-theme 1` file that sets one is refused whole, and so is `appearance_set` with it. A **saved appearance** from before the two tokens is not refused whole for it: replaying it drops that one colour, keeps the theme, scale, text size, reduced motion and legible type, and writes an audit line ("the saved colour muted #666666 is dropped: muted on hover is 4.32:1, under 4.5:1"). A save short on an earlier ground, which was short before too, is still refused whole. The next change to the appearance saves it without the dropped colour.

Type. The same seven styles at the same sizes, so the glyph atlas gains no key. All families are SIL OFL-1.1.

| Style | Font, size/line, weight | Used for |
|---|---|---|
| small | Inter 4.1: 12/16 400 | tray text, tooltip titles, tile names, summaries |
| label | 13/18 500 | window titles, section heads, the pane's title, search-row names |
| body | 14/20 400 | the menu's box, Kai's words |
| mono | JetBrains Mono 2.304: 14/20 400 (bold 700) | your lines, call lines, the `kai>` input |
| mono_title | 12/16 500 | kernel labels, the status row, the clock (tabular cells, so digits never shift) |
| heading, display | Inter 18/24 600; 28/34 600 with `opsz` 32 | Settings |
| legible (setting) | Atkinson Hyperlegible Next and Mono, 1 px larger | low vision |

The rule: system words are mono; the AI's words are sans, in the AI's look; an app's title is sans after a mono kernel label.

Motion. Nothing the user starts waits behind an animation; only AI surfaces move.

| Token | Value | Used for |
|---|---|---|
| instant | 0 ms | typing, caret, focus, window open, close and raise, menus, the pane, hover, tooltips |
| quick | 90 ms, cubic-bezier(0.2, 0, 0, 1), opacity | hover and press on controls |
| state | 160 ms cross-fade | the state line's changes |
| sheet | spring, 0.28 s, bounce 0 (stiffness 503.6, damping 44.9), translate only | a sheet; interruptible, keeps its velocity |
| settle | 480 ms linear opacity | the agent's outline after an action |
| tick | 125 ms steps | the thinking dots, as #17's status bar |
| reduced | all 0; the dots become a seconds counter | the setting, and every `test=` boot |

No blur, translucency or window-content animation, and nothing longer than 0.5 s. Kai Desk adds no motion: the menu, the pane, hover, focus and tooltips are instant, so reduced motion and `test=` boots behave the same and goldens stay pure.

Metrics (`look::metric`, logical px; the grid is 4 px).

| What | Value |
|---|---|
| Spacing scale | 4, 8, 12, 16, 24, 32, 48 |
| Radii | 4 for tiles, hover fills, tooltips, app controls and the card's buttons; 6 for the pane's input; 8 for windows, the menu, the pane and the card; a pill is half its height |
| Taskbar | 48 tall; items 40 with a 4 gap; icons 24 in the bar, 32 on a menu tile, 16 in a title row |
| Flyout margin | 12 from each screen edge, and 12 above the taskbar |
| Window | title bar 32; frame 1; radius 8 (0 when maximized); controls 40x32; resize grabs 6 outside the frame, corners 16; shadow reach 12 |

**Layout.** Every size below is in physical px at scale 1 (§9). One pure function, `deskgeom::Layout::for_screen(w, h, ui)`, will compute every rectangle, so painting, hit-testing, tests and boot cases read one source. 1920x1080 is the design size. 1280x800 keeps every size, and only the pane narrows to 360. 1024x768 uses the compact tray and a 340 px pane that closes when focus leaves it. The menu and the card are the same at every size. The work area is (0, 0, W, H-48). Nothing overlaps in the default state at any of the three sizes, and a test asserts it.

**Windows.**
- Title bar 32, frame 1 px (`line_strong` when focused, `line` otherwise), radius 8 (0 when maximized), resize grabs 6 outside the frame, corners 16. Windows cascade from (48, 52), 32 further for each, n mod 8.
- Controls are 40x32, flush in the title bar and 16 px glyphs, right to left: close, maximize or restore, minimize. At a frame from x 48 to 690 they sit at close 649 to 689, maximize 609 to 649, minimize 569 to 609, y 53 to 85. Hover is `hover`. Close hover is an `error` fill with the glyph in `window` (6.40:1 dark, 6.20:1 light). Inside the frame the close control wins over the corner grab.
- The title row is a 16 px app icon (or a monogram), the kernel's label in `mono_title` `dim`, an 8 px gap, then the app's title in `label`: `text` when focused, `dim` otherwise. The label always comes first (§7).
- The focused window and the flyouts have a shadow of two nine-slice layers reaching 12 px, split from the 6 px grab; a dragged window has none. While the agent acts on a window, a 2 px `ai` outline sits 2 px outside its frame.
- Maximize fills the work area. With the pane open at 1280 wide or more it fills to 12 px left of the pane, and a maximized window is re-fitted when the pane opens or closes.

**The taskbar** is (0, H-48, W, 48), filled `shell` with a 1 px `line` rule along its top edge.
- Items are 40x40 at y H-44 with a 4 px gap. Each hit box covers the full 48 px height, the gaps split between neighbours, so no pixel is dead. Hover and pressed fill is `hover`, radius 4. Icons are 24 px.
- Centred, in order: the Kai button; the pins (terminal, files, notes, editor, monitor, settings, each skipped if not installed); then running windows of apps that are not pinned, in open order. With 7 items the group is 304 wide.

| Size | Bar | 7-item group | Kai button centre |
|---|---|---|---|
| 1920x1080 | (0, 1032, 1920, 48) | x 808 to 1112 | (828, 1056) |
| 1280x800 | (0, 752, 1280, 48) | x 488 to 792 | (508, 776) |
| 1024x768 | (0, 720, 1024, 48) | x 360 to 664 | (380, 744) |

- At most 12 items at 1280 wide and up and 9 at 1024; past that the last slot is "more", which opens All apps. The group never meets the tray: at 1024 it ends at 664 and the tray starts at 848.
- A pin and its windows share one item, matched by the kernel's process label and never by title. With several windows a click activates the most recent; a click on the focused one minimizes it; a click on a minimized one restores it. A pin with no window sends the line "open <name>", the path the Apps sheet takes today.
- A running pill sits 2 px above the item's bottom, 3 px tall with round ends: 6 wide `muted` when running and unfocused, 16 wide `accent` when focused, 4 wide `muted` when minimized. The item behind it takes the `hover` fill, so the pill is never `dim`.
- The Kai button draws the Kai mark in `ai` (`ai_mark` while hovered), a 16 px `ai_mark` pill under it while Thinking or Acting, and a 2 px `ask` ring at radius 8 while an ask card is up. The ring is a second trusted cue: apps cannot paint the taskbar.
- A tooltip comes after 500 ms of hover (a delay, not an animation): 28 tall, `raised`, radius 4, a 1 px `line_strong` frame, 8 px above the taskbar and kept on screen. It shows the kernel's label in `mono_title` `dim`, a 6 px gap, then the title in `small` `text`, at most 320 wide and ending in "…" if cut. The taskbar draws no app text except inside this label-first tooltip.

**The tray** is right-aligned and a pure function of W, so the screendump mask is computed, not written down. At 1280 wide and up, right to left:

| Part (x, y, w, h) | 1920x1080 | 1280x800 | 1024x768 (compact) |
|---|---|---|---|
| Kai corner (W-16, H-48, 16, 48), hit only | (1904, 1032) | (1264, 752) | (1008, 720) |
| Clock slot (W-96, H-48, 80, 48) | (1824, 1032) | (1184, 752) | (928, 720) |
| Memory (W-192, H-44, 88, 40) | (1728, 1036) | (1088, 756) | hidden |
| Kai chip (W-360, H-44, 160, 40); compact (W-168, H-44, 64, 40) | (1560, 1036) | (920, 756) | (856, 724), 64 wide |
| Tray area, the screendump mask | (1552, 1032, 368, 48) | (912, 752, 368, 48) | (848, 720, 176, 48) |

- The Kai corner's hit rectangle contains (W-1, H-1), so a clamped pointer move reaches it. A click opens the pane and focuses its input.
- The clock is two lines in `mono_title`, right-aligned in fixed cells: "14:05" in `text` at y H-40 and "14 Nov UTC" in `muted` at y H-24. It reads the kernel's wall clock and draws nothing if there is none. The slot sits on 16 px columns at every size (columns 114 to 118 at 1920, 74 to 78 at 1280, 58 to 62 at 1024), so even a midnight rollover changes at most five tiles in a row. It says UTC; a time-zone setting is open question 7.
- Memory is a 16 px icon and "2.1 GiB" in `small` `muted`, in 0.1 GiB steps, repainted only when the text changes. Compact mode hides it; it stays in the chip's tooltip.
- The Kai chip is an 8 px dot and `small` text, code-only, from the panel's state, the model's warmth and the bar. The dot is `ok` when ready, a `muted` ring while getting ready, and a `muted` dash with no model; it is never `ask`, and never `dim`, since the chip takes the `hover` fill. The text is "Kai ready", "Kai is getting ready", "Kai thinking 4s", "Kai acting", "Kai done", "Kai refused" or "No model". While the pane is open it shows only the dot and "Kai". A 6 px `ai_mark` badge sits at its top right when Kai said something while the pane was closed. A click toggles the pane. Hover is `hover`, radius 4.

**The Kai menu** (Start in the code) replaces the Apps sheet. It is `raised`, radius 8, with a 1 px `line_strong` frame and a flyout shadow, 640 wide and min(680, H-72) tall, centred, its bottom at H-60.

| Size | Menu rectangle |
|---|---|
| 1920x1080 | (640, 340, 640, 680) |
| 1280x800 | (320, 60, 640, 680) |
| 1024x768 | (192, 28, 640, 680) |

Offsets are from the menu's top left, and the padding is 24.
- **The box** is (24, 24, 592, 40), a pill filled `window` with a 1 px `line_strong` frame, a 16 px search icon at inner x 14 and body text from inner x 40. The placeholder is "Ask Kai or search apps" in `dim`, or "Kai is getting ready · search apps" while the model warms. Focused, it has a 2 px `ai` rule inside its bottom edge and the ember block caret. #856's `<` horizontal scroll is kept.
- **Home.** "Pinned" in `label` at y 88, with an "All apps ›" link in `accent` at the right. A grid of 6 columns by 3 rows of 96x84 cells from (32, 116) to y 368: a 40x40 tile with a 32 px icon, and the name in `small` below, centred and cut to 88 wide. Hover and selection are `hover`, radius 4. "Recent" in `label` at y 384, then up to 5 rows of 40 from y 408: your own last lines (never Kai's words) in `mono` `muted`; a click puts the line back in the box. A footer band (0, 616, 640, 64) in `shell` with a 1 px `line` on top holds the Kai mark at 20 px and the model's state in `small` `muted` at the left, and a 40x40 Settings item at the right.
- **Search** replaces the grid when the box has text: rows of 52 from y 80 to 616 (10 rows). Row 0 is "Ask Kai: <text>" with the Kai mark in `ai_mark`, the text in `mono` `muted`. Then come matching catalog rows: a 24 px icon, the name in `label`, the summary in `small` `muted` (a row takes the `hover` fill, which `dim` is never drawn on), and an Installed or Install tag. The selected row has the `hover` fill, radius 4, and a 3 px `accent` edge, as in All apps; the pointer's hover alone is the fill without the edge, so a selected row stays distinct under the pointer. The default selection is the first app whose name starts with the text (case-insensitive), otherwise "Ask Kai": "notes" then Enter opens Notes. Enter on Ask Kai queues the panel request, closes the menu, and opens and focuses the pane. Enter on an app sends "open <name>" or "install <name>".
- **All apps** is reached by "All apps ›", the `apps` line or the "more" item: a 30 px header, then catalog rows of 52 from y 112 (9 rows visible), in today's order and with today's keys (Up, Down, Enter, Esc). The selected row has a 3 px `accent` edge; ember is Kai's alone.
- Rows are catalog data. They are never queued to the model; only typed text is sent.

**The Kai pane** replaces the `kai>` bar and the history on the desk. It is closed when the desktop starts, at every size. It is `shell`, radius 8, a 1 px `line_strong` frame and a flyout shadow, with its right edge 12 px from the screen's. Its width is 400 at W of 1600 or more, 360 at 1200 or more, and otherwise 340; its x is W-12-width, its y 12 and its height H-72.

| Size | Pane rectangle |
|---|---|
| 1920x1080 | (1508, 12, 400, 1008) |
| 1280x800 | (908, 12, 360, 728) |
| 1024x768 | (672, 12, 340, 696) |

Parts, relative to the pane, with w and h its width and height:
- **Header**, 48 tall: the Kai mark at 20 px at (16, 14); "Kai" in `label` `text` at x 44, then the model's name and state in `small` `muted`, code-only; a 32x32 close item at (w-40, 8); a 1 px `line` rule at y 48.
- **History** (16, 56, w-32, h-160): newest at the bottom, wrapped to its width, clipped to its rectangle so it can never reach the status row, the input, the taskbar or the card. Scrolled by PageUp and PageDown; a new entry snaps back to the bottom unless you have scrolled up. See "The pane's history" below.
- **Status row** (0, h-100, w, 32) with a 1 px `line` on top, in `mono_title`: the states table below, code-only.
- **Input** (12, h-56, w-24, 44), radius 6, `window` with a 1 px `line_strong` frame: `kai>` in bold `ai` mono, then the line (400 bytes at most) with #856's `<` scroll. The placeholder is "Ask Kai, or type Rhai", or "Kai is getting ready" while the model warms. Focused, it has a 2 px `ai` top rule and the ember block caret. A 32x32 send item at its right end becomes a Stop square while Kai is thinking; a click on it acts exactly like Esc.
- The pane floats over windows. Under 1280 wide it is a flyout and closes when focus leaves it. It is not docked: docking needs a work-area protocol, open question 9.

**The ask card** keeps §7's rules and its width: min(W-32, 960), centred, its bottom at H-60. Its height fits its content up to the work height, H-72, which fixes today's cut-off: 1008 at 1080 tall, 728 at 800 and 696 at 768, against 366 at 768 today, so a call filling the whole 2,048-byte ask box shows whole at 1024x768 (480). A call that would not fit even then is refused with a code-only "too long to show on the card" result; the card never shows a cut call. At 1920 it is x 480, at 1280 x 160 and at 1024 x 32, all 960 wide. It is `raised`, radius 8, with a 1 px `line_strong` frame, the 3 px `ask` bar, "Kai asks to run", the 2:00 countdown, mono call lines and a sans reason. Its buttons are 120x32 at radius 4: Don't (n), then Run (y), or "y again" after the first press. The highlight starts on Don't.

**The Kai glyphs** are ours, drawn in code, and replace Lucide.
- Each icon is path geometry on a 24-unit grid with a 2-unit safe margin, stroked 1.5 units with round caps and joins and never under 1 px. A monochrome coverage mask is cached for each icon at 16, 20, 24 and 32 px (about 60 KB in all) and tinted with a token.
- The set: the Kai mark (a ring with a filled spark dot at 45 degrees, grown from today's brand ring, and never a four-pane shape); the apps (terminal, files, notes, editor, monitor, models, settings); search, chat, memory, chevrons, check, stop, send, more and power; the window controls minimize, maximize, restore and close; and a monogram for any other app, the first letter of its kernel label in `label` on a `hover` tile.
- The geometry is authored fresh and licensed with the OS (MPL-2.0). Nothing is traced from Fluent, Segoe MDL2 or Lucide. An icon the toolkit names and the set lacks draws as today's 1 px frame.

**States.** The status row and the chip read their states from 0002's structured `Outcome` and `actlog 1` entries and the model's warmth, never from reply text.

| State | Pane status row | Chip |
|---|---|---|
| idle | the model dot and "Ready", or "Kai is getting ready" in `dim` while the model warms | "Kai ready", "Kai is getting ready" or "No model" |
| listening | the input's 2 px `ai` top rule and the ember block caret | unchanged |
| thinking | "thinking 4s · Esc stops" and a 64x24 Stop item; shown within 100 ms of Enter | "Kai thinking 4s" |
| acting | the call in mono and its target ("Save (button) in Notes"); the outline on that window | "Kai acting" |
| done | an `ok` check and code's result line ("Done: X (was Y)"), with `undo` and `show` links hit where they are drawn | "Kai done" |
| asking | the ask card over everything (§7), and the `ask` ring on the Kai button | unchanged |
| refused | a 3 px `error` bar and the reason | "Kai refused" |

**The pane's history** (the founder, 2026-11-14, after the first VirtualBox test; row `2026-11-14-panel-history`; moved from the desk into the pane by row `2026-11-14-kai-desk`). Each line you type and what came of it shows in the pane's history rectangle, newest at the bottom, as a chat does. The state line stays code's alone. The history alone shows the model's words and a model script's prints, sent by the shell as `panel said`. It draws them as the AI's, never in the system's look: a `Kai` label in `ai`, a 2 px `ai` rule down their left, sans `body` `text`. Your lines are `mono` `muted` and the system's notes `mono` `dim`, as the system's own words are. A line that asks first, or leaves a question open, still goes to the terminal, and the history's note says so. The history is bounded (40 entries, 4,000 bytes each) and drops control characters. It holds no keys of its own, and the card, the status row and the input are never covered by it.

**Keys.** A tap of Super or Ctrl+Esc opens the menu with its box focused. Ctrl+Space, or the Kai corner, opens the pane and focuses its input. Esc in the menu closes it and never interrupts. Esc in the pane's input interrupts only when the panel's own request is running; otherwise it returns focus to the previous window and the pane stays open. Alt+Tab is unchanged, and Ctrl+Alt+F12 stays with the kernel. Whether Super and Ctrl+Esc reach the guest in VirtualBox and QEMU, and past the Terminal's keyboard lease, is unverified (open question 1).

Kept from the accepted look: no blur or translucency, motion at most 0.5 s and nothing animated that the user waits on, the Atkinson pair as the legible setting, and `kai-theme 1`.

### 9. Scale

Every size is in logical px. The scale comes from the EDID physical size when there is one, otherwise from the width (up to 1920 wide 1.0, up to 2560 1.25, up to 3840 2.0), in quarter steps from 1.0 to 3.0; a setting overrides it. At those defaults 1080p, 1440p and 4K give 1920x1080, 2048x1152 and 1920x1080 logical px, so layouts barely change. Native apps render at the exact scale. Foreign and bridged surfaces get an integer scale and are resampled.

Amended 2026-11-14: unchanged at scale 1. The desktop did not yet call `scale::for_screen`, so `deskgeom`, the crate that lays out every rectangle of §8, worked in physical px, which equal logical px at scale 1, until that call was wired.

Amended 2026-11-18: wired. `desktop` calls `scale::for_screen(width, width_mm)` once per claim, with the screen's physical width from `DisplayInfo` (EDID, #976; 0 until the kernel fills it, which gives the width rule). The desk is then built on the *logical* size, the screen's size over the scale (2560x1440 at 1.25 is 2048x1152): `Desk`, `deskgeom`'s layout, hit tests, input (an absolute position spans the logical screen, a motion count is one logical px), the painter and its frame are all logical px and know nothing of the scale. One place makes them physical: `frame::present`, which at scale 1.0 copies the damaged rectangles exactly as before (every golden at 1024x768, 1280x800 and 1920x1080 is unchanged) and above 1.0 resamples each damaged rectangle bilinearly onto the screen (`deskcore::upscale`; the damage grows by the one logical px a sample reads past it, a host test checks it against a full resample). The hardware cursor moves in physical px and keeps its 64x64 image. Native apps draw at the exact scale (added 2026-11-18, the same day): `desktop` tells each client the scale it prefers (`registry.scale`, a new event, sent only when it is not 1x), the client answers with `surface.set_buffer_scale` (a new request, 120 to 360) and draws buffers at it, and the desk sizes the window from the buffer divided by that scale. A window drawn at the screen's scale is a hole in the logical frame (the window colour with its blue bit flipped), and `desktop` writes the window's id into a mask of the frame's size as it paints, so the mask names whose content was painted last at each px (a window not drawn at the scale writes 0). After the resample, `deskcore::overlay::overlay` puts the client's own pixels on the screen only at px where the frame still shows the hole, the mask holds that window's id, and the px lies inside the window's content and its buffer: a window, the taskbar, the pane, a card or the cursor over it stays over it, a client can never reach another window's px whatever its buffer's size or the colour it paints, and its pixels are never resampled. A client that does not know the messages draws at 1x and is resampled with the desk, as before. At 1x neither message is sent and no byte on the wire changes. §9's setting override is not built.

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
| Moving the AI panel | Moderate | Apps are told their work area, and users learn where it is. Done 2026-11-14 (Kai Desk): the panel moved from the bottom bar to the pane, through `deskgeom`'s one layout function, so the next move changes numbers in one crate |
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

1. **The look.** After S05-1's screendumps, do you approve dark-first "Ember Workbench", with the bottom `kai>` line as the AI panel and launcher? Or do you prefer light-first "Daylight Document": flat pages, Atkinson type, an AI column on the right? Amended 2026-11-14: the founder asked for Kai Desk instead (§8), still dark-first, with Kai in the pane and the menu; the founder approved the amendment on 2026-11-15 (#925). Still open: whether Super reaches the guest in VirtualBox and QEMU, and Ctrl+Esc, which a Windows host may take too, stay open until they are checked in the VM, and whether either gets through the Terminal's keyboard lease. Until then the ways to the menu and the pane that always work are the Kai button, the `apps` line, Ctrl+Space and the Kai corner.
2. **Decided 2026-10-22:** the OFL fonts are kept and Unicode-3.0 is in the free class, so Inter, JetBrains Mono and Atkinson Hyperlegible Next ship under OFL-1.1, and the font crates' `yoke` and `zerofrom` are free. (The answer was "keep ... the OFL fonts"; read here as covering these three. Say if it was meant for Noto only.) C libxkbcommon's HPND files would still come back to you, only if ADR 0006 falls back to `lxabi`.
3. **Linux apps and the agent in v0.2.** The agent can open, focus and close the Linux window and use 0006's browser rows, but cannot press buttons inside Linux apps. That needs accessibility inside the guest carried out by the bridge, or synthetic input at the ask tier. Is window-level control plus the browser rows enough for v0.2?
4. **CPU-only graphics.** GOP on the RTX 4070 Ti through v1.0 (0007's question 4) means no blur or glass, 4K that may animate at 30 Hz, and 1440p at most in VirtualBox. Is that acceptable, or should GPU work move earlier, at a cost of years?
5. **Who draws questions.** Ask cards are drawn by the ring-3 compositor, not the kernel; `:apply` stays kernel-drawn. Does that meet your bar for "an app cannot fake a yes"?
6. **A typeface of our own.** Added 2026-11-14. Kai Desk keeps Inter and JetBrains Mono, which are OFL and not Microsoft's, but are not drawn for Kai. A bespoke face is a separate, large job, and renaming an OFL build is allowed only for a font without a Reserved Font Name. Default: keep them.
7. **A time-zone setting.** Added 2026-11-14. The tray clock shows UTC and says so on its date line, since the OS has no zone. A zone is its own decision (where it is stored, and how it is set), not part of `appearance_set`. Default: UTC until it is decided. **Built 2026-11-18, at the founder's default (a fixed offset, no daylight saving, yes):** `time_zone = +05:30` (or `UTC`, `-9`; -12:00 to +14:00 in 15-minute steps) is one more line of the saved `kai-appearance 1` file under `/system/settings`, written only when it is not UTC, so a saved file without it reads as before. The shell sends the file to `desktop` as it does the rest, and a plan-line `time_zone=+2` sets it for a test image. The tray clock then reads the wall time in that offset and its date line names it ("14 Nov UTC+2"; both lines draw smaller together if the longer label needs it). It is not one of the six settings of the agent's `appearance_set` row (`look::appearance::FILE_SETTINGS` is those and `time_zone`), which would change what the model is shown; a Settings page row and the agent's row are the follow-ups.
8. **The wheel for apps.** Added 2026-11-14. The mouse wheel scrolls the pane's history and the menu's lists (#947). An app gets no wheel events until `kai-surface` carries them, which is a `surfabi` change of its own. Default: the desktop's own lists only.
9. **Docking the pane.** Added 2026-11-14. The pane floats over windows, and only a maximized window is fitted beside it, so at 1280 wide a window dragged under it is partly covered while it is open. Docking needs a work-area protocol (the `panel` object is unserved). Default: float, closed at boot.

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
